package main

// Web Push sender (README.md, "Push"; contract: .sbxw-artifacts/remote/API-push.md).
//
// With --push, the daemon writes jobs on this program's stdin and reads one
// answer per job on its stdout. The sidecar encrypts a fixed-shape payload
// (RFC 8291, aes128gcm of RFC 8188), signs a VAPID token (RFC 8292) and posts
// it to an Apple push endpoint. Everything is Go's standard library.
//
// It is not an Internet door: only https://<label>.push.apple.com/<path>,
// only POST, only a body this file builds from a kind letter, a badge
// count and a sandbox name of letters, digits and hyphens, only to public addresses, with TLS verified against the system
// roots, no proxy, no redirect, one request at a time, 60 an hour at most.

import (
	"bufio"
	"bytes"
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/ecdh"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/hkdf"
	"crypto/rand"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/asn1"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/big"
	"net"
	"net/http"
	"net/netip"
	"net/url"
	"os"
	"os/signal"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"
)

// Limits of the push channel.
const (
	pushLineMax     = 4 << 10 // a job or config line, newline excluded
	pushPathMax     = 512     // endpoint path, bytes
	pushEndpointMax = 1024    // whole endpoint URL, bytes
	pushSubMax      = 256     // the VAPID sub URL, bytes
	pushTTLMax      = 86400   // seconds
	pushBadgeMax    = 99
	pushRatePerHour = 60
	pushTimeout     = 10 * time.Second
	pushRespReadMax = 1 << 10
	pushRecordSize  = 4096 // the rs field of the aes128gcm header
	pushNameMax     = 63   // a sandbox name, bytes (sbx's own limit)
	pushPlainPadded = 128  // payload JSON + 0x02 delimiter + zero padding: fits the longest name
	pushJWTLife     = 12 * time.Hour
	pushJWTReuse    = 11 * time.Hour // a token is reused until it is this old
	pushJWTCacheMax = 16
	pushIDMax       = 64
	// pushTopic is the same for every push: Apple keeps one pending push per
	// phone and learns nothing of the kind, which stays inside the encryption.
	pushTopic        = "sbxw"
	pushHeaderLength = 16 + 4 + 1 + 65
)

// The body is always this many bytes: header, padded plaintext, GCM tag.
const pushBodySize = pushHeaderLength + pushPlainPadded + 16

// Answer words, besides an HTTP status code written in decimal.
const (
	ansRefused   = "refused"    // the endpoint or a resolved address broke a rule
	ansTimeout   = "timeout"    // no answer within 10 s
	ansRate      = "rate"       // over 60 jobs in the last hour
	ansBadJob    = "bad_job"    // the line is not a valid job
	ansNetwork   = "network"    // DNS, connect or TLS failed
	ansBadConfig = "bad_config" // the first line was not a valid configuration
	ansStopped   = "stopped"    // the program is stopping
)

var (
	pushIDRE    = regexp.MustCompile(`^[A-Za-z0-9_-]{1,64}$`)
	pushNameRE  = regexp.MustCompile(`^[A-Za-z0-9-]{0,63}$`)
	pushHostRE  = regexp.MustCompile(`^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+push\.apple\.com$`)
	pushPathRE  = regexp.MustCompile(`^/[A-Za-z0-9_\-/.]*$`)
	errRefused  = errors.New("refused")
	errBadJob   = errors.New("bad job")
	b64         = base64.RawURLEncoding
	cgnatPrefix = netip.MustParsePrefix("100.64.0.0/10") // Tailscale addresses
)

// pushEndpoint is an endpoint that passed checkEndpoint: host and path only.
type pushEndpoint struct {
	host string // lower case, ends in .push.apple.com
	path string // starts with /
}

func (e pushEndpoint) url() string    { return "https://" + e.host + e.path }
func (e pushEndpoint) origin() string { return "https://" + e.host }

// checkEndpoint accepts only https://<labels>.push.apple.com[:443]/<path>
// with a plain path and nothing else. The error never quotes the endpoint.
func checkEndpoint(raw string) (pushEndpoint, error) {
	if len(raw) > pushEndpointMax || !strings.HasPrefix(raw, "https://") {
		return pushEndpoint{}, errRefused
	}
	if strings.ContainsAny(raw, "?#@\\%") {
		return pushEndpoint{}, errRefused
	}
	u, err := url.Parse(raw)
	if err != nil || u.Scheme != "https" || u.Opaque != "" || u.User != nil ||
		u.RawQuery != "" || u.ForceQuery || u.Fragment != "" || u.RawFragment != "" {
		return pushEndpoint{}, errRefused
	}
	if strings.HasSuffix(u.Host, ":") {
		return pushEndpoint{}, errRefused
	}
	if p := u.Port(); p != "" && p != "443" {
		return pushEndpoint{}, errRefused
	}
	host := strings.ToLower(u.Hostname())
	if _, err := netip.ParseAddr(host); err == nil {
		return pushEndpoint{}, errRefused
	}
	if len(host) > 253 || strings.HasSuffix(host, ".") || !strings.HasSuffix("."+host, ".push.apple.com") ||
		!pushHostRE.MatchString(host) {
		return pushEndpoint{}, errRefused
	}
	path := u.Path
	if u.RawPath != "" && u.RawPath != u.Path {
		return pushEndpoint{}, errRefused
	}
	if len(path) > pushPathMax || !pushPathRE.MatchString(path) {
		return pushEndpoint{}, errRefused
	}
	for _, seg := range strings.Split(path, "/") {
		if seg == "." || seg == ".." {
			return pushEndpoint{}, errRefused
		}
	}
	return pushEndpoint{host: host, path: path}, nil
}

// publicAddr reports whether a resolved address may be dialled: never the
// Mac itself, a local network, a link-local or unique-local address, the
// tailnet's 100.64.0.0/10, multicast or the unspecified address.
func publicAddr(a netip.Addr) bool {
	a = a.Unmap()
	if !a.IsValid() || a.IsUnspecified() || a.IsLoopback() || a.IsPrivate() ||
		a.IsLinkLocalUnicast() || a.IsLinkLocalMulticast() || a.IsInterfaceLocalMulticast() ||
		a.IsMulticast() {
		return false
	}
	if a.Is4() && (cgnatPrefix.Contains(a) || a == netip.AddrFrom4([4]byte{255, 255, 255, 255}) ||
		a.As4()[0] == 0) {
		return false
	}
	return true
}

// pushConfig is the first line from the daemon.
type pushConfig struct {
	key    *ecdsa.PrivateKey
	pubB64 string // the k= of the Authorization header
	sub    string
}

func parsePushConfig(line []byte) (*pushConfig, error) {
	var c struct {
		VapidPrivate *string `json:"vapid_private"`
		Sub          *string `json:"sub"`
	}
	if err := strictJSON(line, &c); err != nil || c.VapidPrivate == nil || c.Sub == nil {
		return nil, errors.New("bad config")
	}
	raw, err := b64.DecodeString(strings.TrimRight(*c.VapidPrivate, "="))
	if err != nil || len(raw) != 32 {
		return nil, errors.New("bad config")
	}
	key, err := ecdsa.ParseRawPrivateKey(elliptic.P256(), raw)
	if err != nil {
		return nil, errors.New("bad config")
	}
	pub, err := key.PublicKey.Bytes()
	if err != nil {
		return nil, errors.New("bad config")
	}
	if !checkSub(*c.Sub) {
		return nil, errors.New("bad config")
	}
	return &pushConfig{key: key, pubB64: b64.EncodeToString(pub), sub: *c.Sub}, nil
}

// checkSub accepts a plain https URL (decision D5: the project URL).
func checkSub(s string) bool {
	if len(s) > pushSubMax || !strings.HasPrefix(s, "https://") || strings.ContainsAny(s, "\"\\ \t\r\n") {
		return false
	}
	u, err := url.Parse(s)
	return err == nil && u.Host != "" && u.User == nil && u.RawQuery == "" && u.Fragment == ""
}

// strictJSON decodes exactly one object with no unknown field.
func strictJSON(line []byte, v any) error {
	dec := json.NewDecoder(bytes.NewReader(line))
	dec.DisallowUnknownFields()
	if err := dec.Decode(v); err != nil {
		return err
	}
	if dec.More() {
		return errors.New("trailing data")
	}
	var extra json.RawMessage
	if err := dec.Decode(&extra); err != io.EOF {
		return errors.New("trailing data")
	}
	return nil
}

// pushJob is a job that passed parseJob.
type pushJob struct {
	id       string
	endpoint pushEndpoint
	uaPublic *ecdh.PublicKey
	auth     []byte
	kind     string // "p" or "n"
	name     string // the sandbox, [A-Za-z0-9-]{0,63}; may be empty
	lead     bool   // the sandbox is a team leader
	badge    int
	ttl      int
}

// parseJob returns the job, or the job's id (when it could be read) and an
// error: errRefused for an endpoint outside the allow-list, errBadJob else.
func parseJob(line []byte) (*pushJob, string, error) {
	var j struct {
		ID       *string `json:"id"`
		Endpoint *string `json:"endpoint"`
		P256dh   *string `json:"p256dh"`
		Auth     *string `json:"auth"`
		Kind     *string `json:"kind"`
		Name     *string `json:"name"`
		Lead     *bool   `json:"lead"`
		Badge    *int64  `json:"badge"`
		TTL      *int64  `json:"ttl"`
	}
	if err := strictJSON(line, &j); err != nil {
		return nil, idOf(line), errBadJob
	}
	if j.ID == nil || !pushIDRE.MatchString(*j.ID) {
		return nil, "", errBadJob
	}
	id := *j.ID
	if j.Endpoint == nil || j.P256dh == nil || j.Auth == nil || j.Kind == nil || j.Name == nil || j.Lead == nil || j.Badge == nil || j.TTL == nil {
		return nil, id, errBadJob
	}
	if *j.Kind != "p" && *j.Kind != "n" {
		return nil, id, errBadJob
	}
	// Letters, digits and hyphens only: JSON never escapes them, so the
	// plaintext always fits pushPlainPadded.
	if len(*j.Name) > pushNameMax || !pushNameRE.MatchString(*j.Name) {
		return nil, id, errBadJob
	}
	if *j.TTL < 0 {
		return nil, id, errBadJob
	}
	pk, err := b64.DecodeString(strings.TrimRight(*j.P256dh, "="))
	if err != nil || len(pk) != 65 || pk[0] != 4 {
		return nil, id, errBadJob
	}
	ua, err := ecdh.P256().NewPublicKey(pk)
	if err != nil {
		return nil, id, errBadJob
	}
	auth, err := b64.DecodeString(strings.TrimRight(*j.Auth, "="))
	if err != nil || len(auth) != 16 {
		return nil, id, errBadJob
	}
	ep, err := checkEndpoint(*j.Endpoint)
	if err != nil {
		return nil, id, errRefused
	}
	badge := *j.Badge
	if badge < 0 {
		badge = 0
	}
	if badge > pushBadgeMax {
		badge = pushBadgeMax
	}
	ttl := *j.TTL
	if ttl > pushTTLMax {
		ttl = pushTTLMax
	}
	return &pushJob{id: id, endpoint: ep, uaPublic: ua, auth: auth, kind: *j.Kind, name: *j.Name, lead: *j.Lead, badge: int(badge), ttl: int(ttl)}, id, nil
}

// idOf recovers a well-formed id from a line that is otherwise bad, so the
// daemon can match the answer. Never more than the id is kept.
func idOf(line []byte) string {
	var j struct {
		ID string `json:"id"`
	}
	if json.Unmarshal(line, &j) != nil || !pushIDRE.MatchString(j.ID) {
		return ""
	}
	return j.ID
}

// pushPlaintext is the payload, before encryption:
// {"kind":"n","badge":7,"name":"sbxw-2","lead":true}. It is padded to
// pushPlainPadded whatever the name, so its size says nothing of it.
func pushPlaintext(kind string, badge int, name string, lead bool) []byte {
	b, _ := json.Marshal(struct {
		Kind  string `json:"kind"`
		Badge int    `json:"badge"`
		Name  string `json:"name"`
		Lead  bool   `json:"lead"`
	}{kind, badge, name, lead})
	return b
}

// encryptPush is RFC 8291 section 3.4 with one aes128gcm record (RFC 8188),
// the data padded so that data + delimiter + zeros is padTo bytes. asPriv
// and salt are random in production, fixed in the RFC's test vector.
func encryptPush(data []byte, padTo int, ua *ecdh.PublicKey, auth []byte, asPriv *ecdh.PrivateKey, salt []byte, rs uint32) ([]byte, error) {
	if len(data)+1 > padTo || len(salt) != 16 || len(auth) != 16 || int(rs) < padTo+16+1 {
		return nil, errors.New("encrypt: bad sizes")
	}
	ecdhSecret, err := asPriv.ECDH(ua)
	if err != nil {
		return nil, err
	}
	uaPub := ua.Bytes()
	asPub := asPriv.PublicKey().Bytes()
	prkKey, err := hkdf.Extract(sha256.New, ecdhSecret, auth)
	if err != nil {
		return nil, err
	}
	keyInfo := "WebPush: info\x00" + string(uaPub) + string(asPub)
	ikm, err := hkdf.Expand(sha256.New, prkKey, keyInfo, 32)
	if err != nil {
		return nil, err
	}
	prk, err := hkdf.Extract(sha256.New, ikm, salt)
	if err != nil {
		return nil, err
	}
	cek, err := hkdf.Expand(sha256.New, prk, "Content-Encoding: aes128gcm\x00", 16)
	if err != nil {
		return nil, err
	}
	nonce, err := hkdf.Expand(sha256.New, prk, "Content-Encoding: nonce\x00", 12)
	if err != nil {
		return nil, err
	}
	block, err := aes.NewCipher(cek)
	if err != nil {
		return nil, err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return nil, err
	}
	plain := make([]byte, padTo)
	copy(plain, data)
	plain[len(data)] = 0x02 // last-record delimiter, then zeros
	out := make([]byte, 0, pushHeaderLength+padTo+gcm.Overhead())
	out = append(out, salt...)
	out = append(out, byte(rs>>24), byte(rs>>16), byte(rs>>8), byte(rs))
	out = append(out, byte(len(asPub)))
	out = append(out, asPub...)
	// One record, sequence number 0: the nonce is used as derived.
	return gcm.Seal(out, nonce, plain, nil), nil
}

// vapidToken is a cached JWT for one origin.
type vapidToken struct {
	jwt    string
	issued time.Time
}

// vapidJWT signs {"aud": origin, "exp": now+12h, "sub": sub} with ES256.
func vapidJWT(key *ecdsa.PrivateKey, origin, sub string, now time.Time) (string, error) {
	header := b64.EncodeToString([]byte(`{"typ":"JWT","alg":"ES256"}`))
	claims, err := json.Marshal(struct {
		Aud string `json:"aud"`
		Exp int64  `json:"exp"`
		Sub string `json:"sub"`
	}{origin, now.Add(pushJWTLife).Unix(), sub})
	if err != nil {
		return "", err
	}
	signing := header + "." + b64.EncodeToString(claims)
	sum := sha256.Sum256([]byte(signing))
	der, err := ecdsa.SignASN1(rand.Reader, key, sum[:])
	if err != nil {
		return "", err
	}
	var rs struct{ R, S *big.Int }
	if _, err := asn1.Unmarshal(der, &rs); err != nil {
		return "", err
	}
	sig := make([]byte, 64)
	rs.R.FillBytes(sig[:32])
	rs.S.FillBytes(sig[32:])
	return signing + "." + b64.EncodeToString(sig), nil
}

// pushSender sends one job at a time. Its unexported hooks are set only by
// tests (push_test.go); the production constructor leaves them nil.
type pushSender struct {
	cfg     *pushConfig
	now     func() time.Time
	client  *http.Client
	timeout time.Duration

	mu     sync.Mutex
	sent   []time.Time // jobs that reached the network, last hour
	tokens map[string]vapidToken

	// Test hooks, nil in the release binary.
	lookup   func(ctx context.Context, host string) ([]netip.Addr, error)
	addrOK   func(netip.Addr) bool
	dialPort string         // replaces 443 when dialling
	roots    *x509.CertPool // replaces the system roots (still verified)
	ephKey   func() (*ecdh.PrivateKey, error)
}

func newPushSender(cfg *pushConfig) *pushSender {
	s := &pushSender{cfg: cfg, now: time.Now, timeout: pushTimeout, tokens: map[string]vapidToken{}}
	s.client = s.newClient()
	return s
}

// newClient builds the dedicated client: its own transport (not tsnet's
// dialer), no proxy from the environment, TLS verified, no redirect.
func (s *pushSender) newClient() *http.Client {
	tlsConf := &tls.Config{MinVersion: tls.VersionTLS12}
	if s.roots != nil {
		tlsConf.RootCAs = s.roots
	}
	transport := &http.Transport{
		Proxy:                  nil,
		DialContext:            s.dial,
		TLSClientConfig:        tlsConf,
		ForceAttemptHTTP2:      false,
		DisableCompression:     true,
		MaxIdleConns:           2,
		MaxConnsPerHost:        1,
		IdleConnTimeout:        60 * time.Second,
		TLSHandshakeTimeout:    pushTimeout,
		ResponseHeaderTimeout:  pushTimeout,
		MaxResponseHeaderBytes: 16 << 10,
	}
	return &http.Client{
		Transport: transport,
		CheckRedirect: func(*http.Request, []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}
}

type addrRefusedError struct{}

func (addrRefusedError) Error() string { return "address refused" }

// dial resolves the host itself, refuses any non-public answer, and checks
// the address once more in the dialer's Control, just before connect.
func (s *pushSender) dial(ctx context.Context, network, addr string) (net.Conn, error) {
	host, port, err := net.SplitHostPort(addr)
	if err != nil || port != "443" {
		return nil, addrRefusedError{}
	}
	if _, err := checkEndpoint("https://" + host + "/"); err != nil {
		return nil, addrRefusedError{}
	}
	lookup := s.lookup
	if lookup == nil {
		lookup = func(ctx context.Context, h string) ([]netip.Addr, error) {
			return net.DefaultResolver.LookupNetIP(ctx, "ip", h)
		}
	}
	ok := s.addrOK
	if ok == nil {
		ok = publicAddr
	}
	addrs, err := lookup(ctx, host)
	if err != nil {
		return nil, err
	}
	if len(addrs) == 0 {
		return nil, errors.New("no address")
	}
	for _, a := range addrs {
		if !ok(a) {
			return nil, addrRefusedError{}
		}
	}
	if s.dialPort != "" {
		port = s.dialPort
	}
	d := &net.Dialer{
		Timeout: pushTimeout,
		Control: func(_, address string, _ syscall.RawConn) error {
			ap, err := netip.ParseAddrPort(address)
			if err != nil || !ok(ap.Addr()) {
				return addrRefusedError{}
			}
			return nil
		},
	}
	var lastErr error
	for _, a := range addrs {
		c, err := d.DialContext(ctx, "tcp", netip.AddrPortFrom(a.Unmap(), mustPort(port)).String())
		if err == nil {
			return c, nil
		}
		lastErr = err
	}
	return nil, lastErr
}

func mustPort(p string) uint16 {
	n, err := strconv.ParseUint(p, 10, 16)
	if err != nil {
		return 443
	}
	return uint16(n)
}

// token returns the cached JWT for origin, or a new one when the cached one
// is 11 hours old (so never refreshed more often than once an hour).
func (s *pushSender) token(origin string, now time.Time) (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if t, ok := s.tokens[origin]; ok && now.Sub(t.issued) < pushJWTReuse && !now.Before(t.issued) {
		return t.jwt, nil
	}
	jwt, err := vapidJWT(s.cfg.key, origin, s.cfg.sub, now)
	if err != nil {
		return "", err
	}
	if len(s.tokens) >= pushJWTCacheMax {
		s.tokens = map[string]vapidToken{}
	}
	s.tokens[origin] = vapidToken{jwt: jwt, issued: now}
	return jwt, nil
}

// allow counts a job against the hourly cap.
func (s *pushSender) allow(now time.Time) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	kept := s.sent[:0]
	for _, t := range s.sent {
		if now.Sub(t) < time.Hour && !now.Before(t) {
			kept = append(kept, t)
		}
	}
	s.sent = kept
	if len(s.sent) >= pushRatePerHour {
		return false
	}
	s.sent = append(s.sent, now)
	return true
}

// send posts one job and returns the answer's status word or code.
func (s *pushSender) send(ctx context.Context, job *pushJob) string {
	now := s.now()
	if !s.allow(now) {
		return ansRate
	}
	eph := s.ephKey
	if eph == nil {
		eph = func() (*ecdh.PrivateKey, error) { return ecdh.P256().GenerateKey(rand.Reader) }
	}
	asPriv, err := eph()
	if err != nil {
		return ansNetwork
	}
	salt := make([]byte, 16)
	if _, err := rand.Read(salt); err != nil {
		return ansNetwork
	}
	body, err := encryptPush(pushPlaintext(job.kind, job.badge, job.name, job.lead), pushPlainPadded, job.uaPublic, job.auth, asPriv, salt, pushRecordSize)
	if err != nil {
		return ansBadJob
	}
	jwt, err := s.token(job.endpoint.origin(), now)
	if err != nil {
		return ansNetwork
	}
	ctx, cancel := context.WithTimeout(ctx, s.timeout)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, job.endpoint.url(), bytes.NewReader(body))
	if err != nil {
		return ansBadJob
	}
	req.Header = http.Header{}
	req.Header.Set("TTL", strconv.Itoa(job.ttl))
	req.Header.Set("Urgency", "high")
	req.Header.Set("Topic", pushTopic)
	req.Header.Set("Content-Encoding", "aes128gcm")
	req.Header.Set("Content-Type", "application/octet-stream")
	req.Header.Set("Authorization", "vapid t="+jwt+", k="+s.cfg.pubB64)
	req.Header.Set("User-Agent", "sbxw")
	resp, err := s.client.Do(req)
	if err != nil {
		var refused addrRefusedError
		switch {
		case errors.As(err, &refused):
			return ansRefused
		case ctx.Err() != nil && errors.Is(ctx.Err(), context.Canceled):
			return ansStopped
		case ctx.Err() != nil || isTimeout(err):
			return ansTimeout
		default:
			return ansNetwork
		}
	}
	_, _ = io.CopyN(io.Discard, resp.Body, pushRespReadMax)
	_ = resp.Body.Close()
	return strconv.Itoa(resp.StatusCode)
}

func isTimeout(err error) bool {
	var ne net.Error
	return errors.As(err, &ne) && ne.Timeout()
}

// readLine reads one line of at most max bytes. A longer line is read to
// its end and dropped (tooLong); err is io.EOF at the end of input.
func readLine(r *bufio.Reader, max int) (line []byte, tooLong bool, err error) {
	for {
		chunk, isPrefix, e := r.ReadLine()
		if e != nil {
			if len(line) > 0 || tooLong {
				return line, tooLong, nil
			}
			return nil, false, e
		}
		if !tooLong {
			if len(line)+len(chunk) > max {
				tooLong = true
				line = nil
			} else {
				line = append(line, chunk...)
			}
		}
		if !isPrefix {
			return line, tooLong, nil
		}
	}
}

type pushAnswer struct {
	ID     string `json:"id"`
	Status string `json:"status"`
}

// servePush reads the configuration line, then jobs, until in ends, out
// fails or ctx is done. It never panics on input and never stops the proxy.
// newSender lets tests build a sender with their hooks.
func servePush(ctx context.Context, in io.Reader, out io.Writer, logf func(string, ...any), newSender func(*pushConfig) *pushSender) {
	if newSender == nil {
		newSender = newPushSender
	}
	r := bufio.NewReaderSize(in, pushLineMax+2)
	enc := json.NewEncoder(out)
	answer := func(id, status string) bool {
		if err := enc.Encode(pushAnswer{ID: id, Status: status}); err != nil {
			logf("push: answers pipe closed, pushing stopped")
			return false
		}
		return true
	}

	line, tooLong, err := readLine(r, pushLineMax)
	if err != nil {
		logf("push: jobs pipe closed, pushing stopped")
		return
	}
	var sender *pushSender
	if cfg, cerr := parsePushConfig(line); cerr != nil || tooLong {
		logf("push: bad configuration line, every job will be answered %s", ansBadConfig)
		if !answer("", ansBadConfig) {
			return
		}
	} else {
		sender = newSender(cfg)
		logf("push: ready")
	}
	for i := range line {
		line[i] = 0 // the copy that held the private key
	}

	for {
		if ctx.Err() != nil {
			return
		}
		line, tooLong, err := readLine(r, pushLineMax)
		if err != nil {
			logf("push: jobs pipe closed, pushing stopped")
			return
		}
		if len(bytes.TrimSpace(line)) == 0 && !tooLong {
			continue
		}
		var id, status, kind string
		switch {
		case tooLong:
			status = ansBadJob
		case sender == nil:
			id, status = idOf(line), ansBadConfig
		default:
			job, jid, perr := parseJob(line)
			id = jid
			switch {
			case errors.Is(perr, errRefused):
				status = ansRefused
			case perr != nil:
				status = ansBadJob
			default:
				kind = job.kind
				status = sender.send(ctx, job)
			}
		}
		if kind == "" {
			kind = "-"
		}
		logf("push: job %s kind %s status %s", orDash(id), kind, status)
		if !answer(id, status) {
			return
		}
	}
}

func orDash(s string) string {
	if s == "" {
		return "-"
	}
	return s
}

// String hides the key if a config is ever printed by mistake.
func (c *pushConfig) String() string { return "pushConfig{...}" }

// GoString hides it from %#v too.
func (c *pushConfig) GoString() string { return c.String() }

var _ fmt.Stringer = (*pushConfig)(nil)

// startPush runs the push channel on stdin and stdout beside the proxy.
// SIGPIPE is caught so that a closed answers pipe (stdout, fd 1) makes the
// write fail instead of killing the program: pushing stops, the proxy goes on.
func startPush(ctx context.Context, logf func(string, ...any)) {
	signal.Notify(make(chan os.Signal, 1), syscall.SIGPIPE)
	go servePush(ctx, os.Stdin, os.Stdout, logf, nil)
}
