package main

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
	"crypto/x509/pkix"
	"encoding/json"
	"errors"
	"fmt"
	"go/parser"
	"go/token"
	"io"
	"log"
	"math/big"
	"net"
	"net/http"
	"net/http/httptest"
	"net/netip"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync/atomic"
	"syscall"
	"testing"
	"time"
)

func unb64(t *testing.T, s string) []byte {
	t.Helper()
	b, err := b64.DecodeString(strings.Join(strings.Fields(s), ""))
	if err != nil {
		t.Fatalf("base64 %q: %v", s, err)
	}
	return b
}

// RFC 8291 section 5 and appendix A: fixed keys and salt give the published
// header and ciphertext.
func TestPushRFC8291Vector(t *testing.T) {
	plain := unb64(t, "V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24")
	if string(plain) != "When I grow up, I want to be a watermelon" {
		t.Fatalf("plaintext %q", plain)
	}
	asPriv, err := ecdh.P256().NewPrivateKey(unb64(t, "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw"))
	if err != nil {
		t.Fatal(err)
	}
	wantAsPub := unb64(t, "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIg Dll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8")
	if !bytes.Equal(asPriv.PublicKey().Bytes(), wantAsPub) {
		t.Fatal("as_public does not match as_private")
	}
	ua, err := ecdh.P256().NewPublicKey(unb64(t, "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcx aOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"))
	if err != nil {
		t.Fatal(err)
	}
	auth := unb64(t, "BTBZMqHH6r4Tts7J_aSIgg")
	salt := unb64(t, "DGv6ra1nlYgDCS1FRnbzlw")

	// No padding beyond the delimiter, as in the RFC's example.
	got, err := encryptPush(plain, len(plain)+1, ua, auth, asPriv, salt, 4096)
	if err != nil {
		t.Fatal(err)
	}
	header := unb64(t, `DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z 9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml
		mlMoZIIgDll6e3vCYLocInmYWAmS6Tlz AC8wEqKK6PBru3jl7A8`)
	ct := unb64(t, `8pfeW0KbunFT06SuDKoJH9Ql87S1QUrd irN6GcG7sFz1y1sqLgVi1VhjVkHsUoEs bI_0LpXMuGvnzQ`)
	if len(header) != 86 {
		t.Fatalf("header %d bytes", len(header))
	}
	if !bytes.Equal(got[:86], header) {
		t.Errorf("header\n got %s\nwant %s", b64.EncodeToString(got[:86]), b64.EncodeToString(header))
	}
	if !bytes.Equal(got[86:], ct) {
		t.Errorf("ciphertext\n got %s\nwant %s", b64.EncodeToString(got[86:]), b64.EncodeToString(ct))
	}
	// Section 5's whole body. The RFC's example says "Content-Length: 145",
	// but the body it prints, and 86 + 41 + 1 + 16, are 144 bytes.
	body := unb64(t, `DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml
		mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT
		pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN`)
	if len(body) != 144 || !bytes.Equal(got, body) {
		t.Errorf("body differs from RFC 8291 section 5 (%d bytes)", len(body))
	}
	// The receiver's side of the same vector decrypts it.
	uaPriv, err := ecdh.P256().NewPrivateKey(unb64(t, "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94"))
	if err != nil {
		t.Fatal(err)
	}
	if dec, err := decryptPush(got, uaPriv, auth); err != nil || !bytes.Equal(dec, plain) {
		t.Errorf("decrypt: %q %v", dec, err)
	}
}

// decryptPush is the user agent's side (RFC 8291 3.4, RFC 8188 2), for tests.
func decryptPush(body []byte, uaPriv *ecdh.PrivateKey, auth []byte) ([]byte, error) {
	if len(body) < 21 {
		return nil, errors.New("short")
	}
	salt := body[:16]
	rs := int(body[16])<<24 | int(body[17])<<16 | int(body[18])<<8 | int(body[19])
	idlen := int(body[20])
	if len(body) < 21+idlen {
		return nil, errors.New("short keyid")
	}
	asPub, err := ecdh.P256().NewPublicKey(body[21 : 21+idlen])
	if err != nil {
		return nil, err
	}
	record := body[21+idlen:]
	if len(record) > rs {
		return nil, errors.New("more than one record")
	}
	secret, err := uaPriv.ECDH(asPub)
	if err != nil {
		return nil, err
	}
	prkKey, _ := hkdf.Extract(sha256.New, secret, auth)
	ikm, _ := hkdf.Expand(sha256.New, prkKey, "WebPush: info\x00"+string(uaPriv.PublicKey().Bytes())+string(asPub.Bytes()), 32)
	prk, _ := hkdf.Extract(sha256.New, ikm, salt)
	cek, _ := hkdf.Expand(sha256.New, prk, "Content-Encoding: aes128gcm\x00", 16)
	nonce, _ := hkdf.Expand(sha256.New, prk, "Content-Encoding: nonce\x00", 12)
	block, _ := aes.NewCipher(cek)
	gcm, _ := cipher.NewGCM(block)
	plain, err := gcm.Open(nil, nonce, record, nil)
	if err != nil {
		return nil, err
	}
	i := len(plain) - 1
	for i >= 0 && plain[i] == 0 {
		i--
	}
	if i < 0 || plain[i] != 2 {
		return nil, errors.New("bad padding delimiter")
	}
	return plain[:i], nil
}

func newUA(t *testing.T) (*ecdh.PrivateKey, []byte) {
	t.Helper()
	k, err := ecdh.P256().GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	auth := make([]byte, 16)
	rand.Read(auth)
	return k, auth
}

func TestPushRoundTripAndFixedSize(t *testing.T) {
	ua, auth := newUA(t)
	long := strings.Repeat("x", pushNameMax)
	for _, c := range []struct {
		kind  string
		badge int
		name  string
		lead  bool
		want  string
	}{
		{"p", 0, "", false, `{"kind":"p","badge":0,"name":"","lead":false}`},
		{"m", 2, "sbxw-2", false, `{"kind":"m","badge":2,"name":"sbxw-2","lead":false}`},
		{"m", 99, long, false, `{"kind":"m","badge":99,"name":"` + long + `","lead":false}`},
		{"n", 7, "a", true, `{"kind":"n","badge":7,"name":"a","lead":true}`},
		{"c", 3, "sbxw-2", true, `{"kind":"c","badge":3,"name":"sbxw-2","lead":true}`},
		{"c", 0, long, false, `{"kind":"c","badge":0,"name":"` + long + `","lead":false}`},
		{"n", 1, "sbxw-2", false, `{"kind":"n","badge":1,"name":"sbxw-2","lead":false}`},
		{"p", 99, long, false, `{"kind":"p","badge":99,"name":"` + long + `","lead":false}`},
	} {
		as, _ := ecdh.P256().GenerateKey(rand.Reader)
		salt := make([]byte, 16)
		rand.Read(salt)
		body, err := encryptPush(pushPlaintext(c.kind, c.badge, c.name, c.lead), pushPlainPadded, ua.PublicKey(), auth, as, salt, pushRecordSize)
		if err != nil {
			t.Fatal(err)
		}
		// One size on the wire for a short name, a long one and none.
		if len(body) != pushBodySize || pushBodySize != 230 {
			t.Errorf("%s/%q: body %d bytes, want %d (230)", c.kind, c.name, len(body), pushBodySize)
		}
		got, err := decryptPush(body, ua, auth)
		if err != nil || string(got) != c.want {
			t.Errorf("%s/%d: decrypted %q %v, want %s", c.kind, c.badge, got, err, c.want)
		}
		// A wrong auth secret does not decrypt.
		bad := append([]byte(nil), auth...)
		bad[0] ^= 1
		if _, err := decryptPush(body, ua, bad); err == nil {
			t.Error("decrypted with a wrong auth secret")
		}
	}
	// The largest payload still fits the fixed size.
	if n := len(pushPlaintext("n", pushBadgeMax, long, false)); n+1 > pushPlainPadded {
		t.Errorf("largest payload %d bytes does not fit %d", n, pushPlainPadded)
	}
}

func testVapid(t *testing.T) *pushConfig {
	t.Helper()
	k, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	raw, err := k.Bytes()
	if err != nil {
		t.Fatal(err)
	}
	line, _ := json.Marshal(map[string]string{"vapid_private": b64.EncodeToString(raw), "sub": "https://github.com/gallettom/sbxw"})
	cfg, err := parsePushConfig(line)
	if err != nil {
		t.Fatal(err)
	}
	return cfg
}

func TestPushVapidJWT(t *testing.T) {
	cfg := testVapid(t)
	now := time.Unix(1_800_000_000, 0)
	jwt, err := vapidJWT(cfg.key, "https://web.push.apple.com", cfg.sub, now)
	if err != nil {
		t.Fatal(err)
	}
	parts := strings.Split(jwt, ".")
	if len(parts) != 3 {
		t.Fatalf("%d parts", len(parts))
	}
	if h := string(unb64(t, parts[0])); h != `{"typ":"JWT","alg":"ES256"}` {
		t.Errorf("header %s", h)
	}
	var claims map[string]any
	if err := json.Unmarshal(unb64(t, parts[1]), &claims); err != nil {
		t.Fatal(err)
	}
	if len(claims) != 3 || claims["aud"] != "https://web.push.apple.com" || claims["sub"] != "https://github.com/gallettom/sbxw" {
		t.Errorf("claims %v", claims)
	}
	exp := int64(claims["exp"].(float64))
	if exp != now.Add(12*time.Hour).Unix() || exp-now.Unix() > 24*3600 {
		t.Errorf("exp %d, now %d", exp, now.Unix())
	}
	sig := unb64(t, parts[2])
	if len(sig) != 64 {
		t.Fatalf("signature %d bytes", len(sig))
	}
	sum := sha256.Sum256([]byte(parts[0] + "." + parts[1]))
	r, s := new(big.Int).SetBytes(sig[:32]), new(big.Int).SetBytes(sig[32:])
	pub, err := ecdsa.ParseUncompressedPublicKey(elliptic.P256(), unb64(t, cfg.pubB64))
	if err != nil {
		t.Fatal(err)
	}
	if !ecdsa.Verify(pub, sum[:], r, s) {
		t.Error("signature does not verify with the k= public key")
	}
	other, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if ecdsa.Verify(&other.PublicKey, sum[:], r, s) {
		t.Error("signature verifies with another key")
	}
}

func TestPushVapidAudIsOriginOnly(t *testing.T) {
	ep, err := checkEndpoint("https://web.push.apple.com:443/QGx-abc_DEF/123")
	if err != nil {
		t.Fatal(err)
	}
	if ep.origin() != "https://web.push.apple.com" {
		t.Errorf("origin %q", ep.origin())
	}
	if ep.url() != "https://web.push.apple.com/QGx-abc_DEF/123" {
		t.Errorf("url %q", ep.url())
	}
}

func TestPushVapidCache(t *testing.T) {
	cfg := testVapid(t)
	s := newPushSender(cfg)
	t0 := time.Unix(1_800_000_000, 0)
	a, _ := s.token("https://web.push.apple.com", t0)
	for _, d := range []time.Duration{time.Minute, 59 * time.Minute, time.Hour, 10*time.Hour + 59*time.Minute} {
		if b, _ := s.token("https://web.push.apple.com", t0.Add(d)); b != a {
			t.Errorf("refreshed after %v", d)
		}
	}
	b, _ := s.token("https://web.push.apple.com", t0.Add(11*time.Hour))
	if b == a {
		t.Error("not refreshed after 11 h (it expires at 12 h)")
	}
	// A clock that goes back does not reuse a token from the future.
	c, _ := s.token("https://web.push.apple.com", t0)
	if c == b {
		t.Error("reused a token issued later than now")
	}
	o, _ := s.token("https://other.push.apple.com", t0)
	if o == c {
		t.Error("one token for two origins")
	}
	for i := 0; i < 40; i++ {
		s.token(fmt.Sprintf("https://h%d.push.apple.com", i), t0)
	}
	if len(s.tokens) > pushJWTCacheMax {
		t.Errorf("cache holds %d", len(s.tokens))
	}
}

func TestPushEndpointAllowList(t *testing.T) {
	long := "https://web.push.apple.com/" + strings.Repeat("a", 512)
	okLong := "https://web.push.apple.com/" + strings.Repeat("a", 511)
	cases := []struct {
		name, url string
		ok        bool
	}{
		{"apple web push", "https://web.push.apple.com/QGx-abc_DEF.9/x", true},
		{"other apple push host", "https://api.push.apple.com/abc", true},
		{"explicit 443", "https://web.push.apple.com:443/abc", true},
		{"upper case host", "https://WEB.PUSH.APPLE.COM/abc", true},
		{"path of 512", okLong, true},
		{"http", "http://web.push.apple.com/abc", false},
		{"upper case scheme", "HTTPS://web.push.apple.com/abc", false},
		{"bare push.apple.com", "https://push.apple.com/abc", false},
		{"suffix trick", "https://push.apple.com.evil.example/abc", false},
		{"no dot before push", "https://evilpush.apple.com/abc", false},
		{"trailing dot", "https://web.push.apple.com./abc", false},
		{"user info", "https://user@web.push.apple.com/abc", false},
		{"user and password", "https://u:p@web.push.apple.com/abc", false},
		{"port 8443", "https://web.push.apple.com:8443/abc", false},
		{"port 80", "https://web.push.apple.com:80/abc", false},
		{"empty port", "https://web.push.apple.com:/abc", false},
		{"ipv4 literal", "https://17.0.0.1/abc", false},
		{"ipv6 literal", "https://[::1]/abc", false},
		{"ipv6 literal 443", "https://[2001:db8::1]:443/abc", false},
		{"path over 512", long, false},
		{"path with percent", "https://web.push.apple.com/a%2Fb", false},
		{"path with space", "https://web.push.apple.com/a b", false},
		{"path with tilde", "https://web.push.apple.com/a~b", false},
		{"path with colon", "https://web.push.apple.com/a:b", false},
		{"dot dot segment", "https://web.push.apple.com/a/../b", false},
		{"query", "https://web.push.apple.com/abc?x=1", false},
		{"empty query", "https://web.push.apple.com/abc?", false},
		{"fragment", "https://web.push.apple.com/abc#x", false},
		{"backslash", "https://web.push.apple.com\\@evil.example/abc", false},
		{"percent in host", "https://web%2epush.apple.com/abc", false},
		{"underscore label", "https://we_b.push.apple.com/abc", false},
		{"empty", "", false},
		{"no scheme", "web.push.apple.com/abc", false},
		{"opaque", "https:web.push.apple.com/abc", false},
		{"too long", "https://web.push.apple.com/" + strings.Repeat("a", 1100), false},
		{"control char", "https://web.push.apple.com/a\nb", false},
		{"empty label", "https://web..push.apple.com/abc", false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			_, err := checkEndpoint(c.url)
			if (err == nil) != c.ok {
				t.Errorf("%q: err %v, want ok=%v", c.url, err, c.ok)
			}
			if err != nil && strings.Contains(err.Error(), "apple") {
				t.Errorf("error quotes the endpoint: %v", err)
			}
		})
	}
}

func TestPushPublicAddr(t *testing.T) {
	cases := []struct {
		addr string
		ok   bool
	}{
		{"17.57.144.10", true},
		{"2620:149:a44::10", true},
		{"127.0.0.1", false},
		{"127.8.9.10", false},
		{"::1", false},
		{"::ffff:127.0.0.1", false},
		{"10.1.2.3", false},
		{"172.16.0.1", false},
		{"192.168.1.1", false},
		{"169.254.169.254", false},
		{"fe80::1", false},
		{"fc00::1", false},
		{"fd7a:115c:a1e0::1", false},
		{"100.64.0.1", false},
		{"100.101.102.103", false},
		{"0.0.0.0", false},
		{"0.1.2.3", false},
		{"::", false},
		{"224.0.0.1", false},
		{"ff02::1", false},
		{"255.255.255.255", false},
		{"::ffff:10.0.0.1", false},
	}
	for _, c := range cases {
		if got := publicAddr(netip.MustParseAddr(c.addr)); got != c.ok {
			t.Errorf("%s: %v, want %v", c.addr, got, c.ok)
		}
	}
}

// A job whose endpoint resolves to a non-public address is refused before
// any connection: the test resolver answers, nothing listens anywhere.
func TestPushResolvedAddressRefused(t *testing.T) {
	cfg := testVapid(t)
	for _, a := range []string{"127.0.0.1", "::1", "10.0.0.5", "192.168.1.20", "169.254.169.254", "fd7a:115c:a1e0::5", "100.100.100.100", "0.0.0.0", "fe80::2"} {
		var dialled atomic.Int32
		s := newPushSender(cfg)
		s.lookup = func(context.Context, string) ([]netip.Addr, error) {
			dialled.Add(1)
			return []netip.Addr{netip.MustParseAddr("17.57.144.10"), netip.MustParseAddr(a)}, nil
		}
		job := testJob(t, "https://web.push.apple.com/abc")
		if got := s.send(context.Background(), job); got != ansRefused {
			t.Errorf("%s: %q, want refused", a, got)
		}
		if dialled.Load() == 0 {
			t.Errorf("%s: the resolver hook was not used", a)
		}
	}
}

func testJob(t *testing.T, endpoint string) *pushJob {
	t.Helper()
	ua, auth := newUA(t)
	line := jobLine("j1", endpoint, ua, auth, "p", 3, 3600)
	job, _, err := parseJob(line)
	if err != nil {
		t.Fatal(err)
	}
	return job
}

func jobLine(id, endpoint string, ua *ecdh.PrivateKey, auth []byte, kind string, badge, ttl int) []byte {
	b, _ := json.Marshal(map[string]any{
		"id": id, "endpoint": endpoint, "p256dh": b64.EncodeToString(ua.PublicKey().Bytes()),
		"auth": b64.EncodeToString(auth), "kind": kind, "name": "sbxw-2", "lead": true, "badge": badge, "ttl": ttl,
	})
	return b
}

// fakeApple is a TLS server whose certificate names web.push.apple.com,
// signed by a test CA that only the test hook trusts.
type fakeApple struct {
	srv   *httptest.Server
	roots *x509.CertPool
	port  string
}

func newFakeApple(t *testing.T, h http.Handler) *fakeApple {
	t.Helper()
	caKey, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	ca := &x509.Certificate{
		SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "test CA"},
		NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(time.Hour),
		IsCA: true, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign,
	}
	caDER, err := x509.CreateCertificate(rand.Reader, ca, ca, &caKey.PublicKey, caKey)
	if err != nil {
		t.Fatal(err)
	}
	caCert, _ := x509.ParseCertificate(caDER)
	leafKey, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	leaf := &x509.Certificate{
		SerialNumber: big.NewInt(2), Subject: pkix.Name{CommonName: "web.push.apple.com"},
		DNSNames:  []string{"web.push.apple.com"},
		NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(time.Hour),
		KeyUsage: x509.KeyUsageDigitalSignature, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
	}
	leafDER, err := x509.CreateCertificate(rand.Reader, leaf, caCert, &leafKey.PublicKey, caKey)
	if err != nil {
		t.Fatal(err)
	}
	srv := httptest.NewUnstartedServer(h)
	srv.Config.ErrorLog = log.New(io.Discard, "", 0) // expected handshake failures
	srv.TLS = &tls.Config{Certificates: []tls.Certificate{{Certificate: [][]byte{leafDER}, PrivateKey: leafKey}}}
	srv.StartTLS()
	t.Cleanup(srv.Close)
	roots := x509.NewCertPool()
	roots.AddCert(caCert)
	_, port, _ := net.SplitHostPort(srv.Listener.Addr().String())
	return &fakeApple{srv: srv, roots: roots, port: port}
}

// hooked builds a sender that reaches the fake server, through the test
// hooks only: resolve to loopback, allow loopback, dial its port, trust its CA.
func (f *fakeApple) hooked(cfg *pushConfig) *pushSender {
	s := &pushSender{cfg: cfg, now: time.Now, timeout: pushTimeout, tokens: map[string]vapidToken{}}
	s.lookup = func(context.Context, string) ([]netip.Addr, error) {
		return []netip.Addr{netip.MustParseAddr("127.0.0.1")}, nil
	}
	s.addrOK = func(a netip.Addr) bool { return a.IsLoopback() }
	s.dialPort = f.port
	s.roots = f.roots
	s.client = s.newClient()
	return s
}

func TestPushSendExactRequest(t *testing.T) {
	cfg := testVapid(t)
	ua, auth := newUA(t)
	type seen struct {
		method, path, host string
		header             http.Header
		body               []byte
		contentLength      int64
	}
	got := make(chan seen, 1)
	f := newFakeApple(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		got <- seen{r.Method, r.URL.RequestURI(), r.Host, r.Header.Clone(), b, r.ContentLength}
		w.WriteHeader(201)
		io.WriteString(w, strings.Repeat("x", 5000)) // more than 1 KiB: read in part, dropped
	}))
	s := f.hooked(cfg)
	t.Setenv("HTTPS_PROXY", "http://127.0.0.1:1") // ignored: no proxy, ever
	t.Setenv("https_proxy", "http://127.0.0.1:1")
	job, _, err := parseJob(jobLine("a1", "https://web.push.apple.com/QGx-abc_DEF/9", ua, auth, "n", 250, 999999))
	if err != nil {
		t.Fatal(err)
	}
	if st := s.send(context.Background(), job); st != "201" {
		t.Fatalf("status %q", st)
	}
	r := <-got
	if r.method != "POST" || r.path != "/QGx-abc_DEF/9" || r.host != "web.push.apple.com" {
		t.Errorf("request %s %s host %s", r.method, r.path, r.host)
	}
	want := map[string]string{
		"Ttl": "86400", "Urgency": "high", "Topic": "sbxw", "Content-Encoding": "aes128gcm",
		"Content-Type": "application/octet-stream", "User-Agent": "sbxw",
		"Content-Length": strconv.Itoa(pushBodySize),
	}
	var names []string
	for k := range r.header {
		names = append(names, k)
	}
	sort.Strings(names)
	for _, k := range names {
		if k == "Authorization" {
			continue
		}
		if w, ok := want[k]; !ok || r.header.Get(k) != w {
			t.Errorf("header %s: %q (want %q)", k, r.header.Get(k), w)
		}
	}
	if len(r.header) != len(want)+1 {
		t.Errorf("headers %v", names)
	}
	authz := r.header.Get("Authorization")
	if !strings.HasPrefix(authz, "vapid t=") || !strings.HasSuffix(authz, ", k="+cfg.pubB64) {
		t.Errorf("Authorization %q", authz)
	}
	if r.contentLength != pushBodySize || len(r.body) != pushBodySize {
		t.Errorf("body %d bytes, Content-Length %d, want %d", len(r.body), r.contentLength, pushBodySize)
	}
	plain, err := decryptPush(r.body, ua, auth)
	if err != nil || string(plain) != `{"kind":"n","badge":99,"name":"sbxw-2","lead":true}` {
		t.Errorf("payload %q %v", plain, err)
	}
}

func TestPushTLSVerified(t *testing.T) {
	f := newFakeApple(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(201) }))
	s := f.hooked(testVapid(t))
	s.roots = nil // the system roots do not know the test CA
	s.client = s.newClient()
	if st := s.send(context.Background(), testJob(t, "https://web.push.apple.com/abc")); st != ansNetwork {
		t.Errorf("unverified certificate: %q, want network", st)
	}
	// A certificate for another name is refused too, even from a trusted CA.
	s = f.hooked(testVapid(t))
	if st := s.send(context.Background(), testJob(t, "https://api.push.apple.com/abc")); st != ansNetwork {
		t.Errorf("certificate for another host: %q, want network", st)
	}
}

func TestPushReleaseSenderHasNoHooks(t *testing.T) {
	s := newPushSender(testVapid(t))
	if s.lookup != nil || s.addrOK != nil || s.dialPort != "" || s.roots != nil || s.ephKey != nil {
		t.Error("the production sender has a test hook set")
	}
	tr := s.client.Transport.(*http.Transport)
	if tr.Proxy != nil || tr.TLSClientConfig.InsecureSkipVerify || tr.TLSClientConfig.RootCAs != nil {
		t.Error("transport: proxy, skipped verification or replaced roots")
	}
	if tr.DialContext == nil || s.client.CheckRedirect == nil || s.timeout != 10*time.Second {
		t.Error("dialer, redirect rule or timeout missing")
	}
	// No file of the release build skips TLS verification.
	src, _ := filepath.Glob("*.go")
	for _, f := range src {
		if strings.HasSuffix(f, "_test.go") {
			continue
		}
		b, _ := os.ReadFile(f)
		if bytes.Contains(b, []byte("InsecureSkipVerify")) {
			t.Errorf("%s mentions InsecureSkipVerify", f)
		}
	}
}

// The production dialer refuses a port other than 443 and a host outside
// the allow-list, whatever the request names.
func TestPushDialRefusesOtherTargets(t *testing.T) {
	s := newPushSender(testVapid(t))
	for _, addr := range []string{"web.push.apple.com:80", "evil.example:443", "127.0.0.1:443", "[::1]:443"} {
		if _, err := s.dial(context.Background(), "tcp", addr); !errors.As(err, new(addrRefusedError)) {
			t.Errorf("%s: %v, want refused", addr, err)
		}
	}
}

func TestPushRedirectNotFollowed(t *testing.T) {
	var other atomic.Int32
	f := newFakeApple(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/other" {
			other.Add(1)
			w.WriteHeader(201)
			return
		}
		http.Redirect(w, r, "https://web.push.apple.com/other", http.StatusTemporaryRedirect)
	}))
	s := f.hooked(testVapid(t))
	if st := s.send(context.Background(), testJob(t, "https://web.push.apple.com/abc")); st != "307" {
		t.Errorf("status %q, want 307 answered as is", st)
	}
	if other.Load() != 0 {
		t.Error("the redirect was followed")
	}
}

func TestPushTimeout(t *testing.T) {
	release := make(chan struct{})
	f := newFakeApple(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		select {
		case <-release:
		case <-r.Context().Done():
		}
	}))
	defer close(release)
	s := f.hooked(testVapid(t))
	s.timeout = 300 * time.Millisecond
	start := time.Now()
	if st := s.send(context.Background(), testJob(t, "https://web.push.apple.com/abc")); st != ansTimeout {
		t.Errorf("status %q, want timeout", st)
	}
	if d := time.Since(start); d > 3*time.Second {
		t.Errorf("took %v", d)
	}
	// Stopping the program cancels a request in flight.
	ctx, cancel := context.WithCancel(context.Background())
	s.timeout = 5 * time.Second
	go func() { time.Sleep(200 * time.Millisecond); cancel() }()
	if st := s.send(ctx, testJob(t, "https://web.push.apple.com/abc")); st != ansStopped {
		t.Errorf("status %q, want stopped", st)
	}
}

func TestPushRateCap(t *testing.T) {
	s := newPushSender(testVapid(t))
	now := time.Unix(1_800_000_000, 0)
	s.now = func() time.Time { return now }
	s.lookup = func(context.Context, string) ([]netip.Addr, error) {
		return []netip.Addr{netip.MustParseAddr("127.0.0.1")}, nil // refused: no network
	}
	job := testJob(t, "https://web.push.apple.com/abc")
	for i := 0; i < 60; i++ {
		if st := s.send(context.Background(), job); st != ansRefused {
			t.Fatalf("job %d: %q", i, st)
		}
		now = now.Add(time.Second)
	}
	if st := s.send(context.Background(), job); st != ansRate {
		t.Errorf("61st job in an hour: %q, want rate", st)
	}
	now = now.Add(59*time.Minute + 1*time.Second) // the first job is now an hour old
	if st := s.send(context.Background(), job); st == ansRate {
		t.Error("still capped once the first job is an hour old")
	}
}

func TestPushOneRequestAtATime(t *testing.T) {
	var cur, max atomic.Int32
	var topics atomic.Int32
	f := newFakeApple(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Topic") != pushTopic {
			topics.Add(1)
		}
		n := cur.Add(1)
		if n > max.Load() {
			max.Store(n)
		}
		time.Sleep(50 * time.Millisecond)
		cur.Add(-1)
		w.WriteHeader(201)
	}))
	cfg := testVapid(t)
	ua, auth := newUA(t)
	var in bytes.Buffer
	in.Write(configLine(t, cfg))
	for i := 0; i < 4; i++ {
		in.Write(jobLine(fmt.Sprintf("j%d", i), "https://web.push.apple.com/abc", ua, auth, "p", i, 60))
		in.WriteByte('\n')
	}
	var out bytes.Buffer
	servePush(context.Background(), &in, &out, func(string, ...any) {}, f.hooked)
	answers := parseAnswers(t, out.String())
	if len(answers) != 4 {
		t.Fatalf("answers %v", answers)
	}
	for i, a := range answers {
		if a.ID != fmt.Sprintf("j%d", i) || a.Status != "201" {
			t.Errorf("answer %d: %+v", i, a)
		}
	}
	if max.Load() != 1 {
		t.Errorf("%d requests at once", max.Load())
	}
	if topics.Load() != 0 {
		t.Errorf("%d pushes with a Topic other than %q", topics.Load(), pushTopic)
	}
}

func configLine(t *testing.T, cfg *pushConfig) []byte {
	t.Helper()
	raw, err := cfg.key.Bytes()
	if err != nil {
		t.Fatal(err)
	}
	b, _ := json.Marshal(map[string]string{"vapid_private": b64.EncodeToString(raw), "sub": cfg.sub})
	return append(b, '\n')
}

func parseAnswers(t *testing.T, s string) []pushAnswer {
	t.Helper()
	var out []pushAnswer
	for _, l := range strings.Split(strings.TrimSpace(s), "\n") {
		if l == "" {
			continue
		}
		var a pushAnswer
		dec := json.NewDecoder(strings.NewReader(l))
		dec.DisallowUnknownFields()
		if err := dec.Decode(&a); err != nil {
			t.Fatalf("answer line %q: %v", l, err)
		}
		out = append(out, a)
	}
	return out
}

func TestPushConfigLine(t *testing.T) {
	cfg := testVapid(t)
	raw, _ := cfg.key.Bytes()
	k := b64.EncodeToString(raw)
	for name, line := range map[string]string{
		"not json":       "hello",
		"missing sub":    `{"vapid_private":"` + k + `"}`,
		"missing key":    `{"sub":"https://example.org"}`,
		"short key":      `{"vapid_private":"AAAA","sub":"https://example.org"}`,
		"zero key":       `{"vapid_private":"` + b64.EncodeToString(make([]byte, 32)) + `","sub":"https://example.org"}`,
		"mailto sub":     `{"vapid_private":"` + k + `","sub":"mailto:a@example.org"}`,
		"http sub":       `{"vapid_private":"` + k + `","sub":"http://example.org"}`,
		"sub with quote": `{"vapid_private":"` + k + `","sub":"https://exa\"mple.org"}`,
		"unknown field":  `{"vapid_private":"` + k + `","sub":"https://example.org","x":1}`,
		"two objects":    `{"vapid_private":"` + k + `","sub":"https://example.org"}{}`,
	} {
		if _, err := parsePushConfig([]byte(line)); err == nil {
			t.Errorf("%s: accepted", name)
		} else if strings.Contains(err.Error(), k) {
			t.Errorf("%s: error quotes the key", name)
		}
	}
	if fmt.Sprintf("%v %+v %#v %s", cfg, cfg, cfg, cfg) != strings.Repeat("pushConfig{...} ", 3)+"pushConfig{...}" {
		t.Error("a printed config shows its fields")
	}
}

func TestPushAcceptsKindC(t *testing.T) {
	ua, auth := newUA(t)
	for _, k := range []string{"p", "m", "n", "c"} {
		line := jobLine("x1", "https://web.push.apple.com/abc", ua, auth, k, 1, 60)
		if _, _, err := parseJob(line); err != nil {
			t.Errorf("kind %s refused: %v", k, err)
		}
	}
	for _, k := range []string{"C", "cc", "x", ""} {
		line := jobLine("x1", "https://web.push.apple.com/abc", ua, auth, k, 1, 60)
		if _, _, err := parseJob(line); err == nil {
			t.Errorf("kind %q accepted", k)
		}
	}
}

func TestPushAcceptsKindM(t *testing.T) {
	ua, auth := newUA(t)
	for _, k := range []string{"p", "m", "n"} {
		line := jobLine("x1", "https://web.push.apple.com/abc", ua, auth, k, 1, 60)
		if _, _, err := parseJob(line); err != nil {
			t.Errorf("kind %s refused: %v", k, err)
		}
	}
}

func TestPushMalformedJobs(t *testing.T) {
	cfg := testVapid(t)
	ua, auth := newUA(t)
	good := func(mut func(m map[string]any)) string {
		m := map[string]any{
			"id": "x1", "endpoint": "https://web.push.apple.com/abc", "p256dh": b64.EncodeToString(ua.PublicKey().Bytes()),
			"auth": b64.EncodeToString(auth), "kind": "p", "name": "sbxw-2", "lead": false, "badge": 1, "ttl": 60,
		}
		mut(m)
		b, _ := json.Marshal(m)
		return string(b)
	}
	cases := []struct {
		name, line, id, status string
	}{
		{"not json", "{{{", "", ansBadJob},
		{"array", "[1,2]", "", ansBadJob},
		{"unknown field", good(func(m map[string]any) { m["body"] = "hi" }), "x1", ansBadJob},
		{"header field", good(func(m map[string]any) { m["headers"] = map[string]string{"X": "y"} }), "x1", ansBadJob},
		{"no id", good(func(m map[string]any) { delete(m, "id") }), "", ansBadJob},
		{"bad id", good(func(m map[string]any) { m["id"] = "a b\n" }), "", ansBadJob},
		{"long id", good(func(m map[string]any) { m["id"] = strings.Repeat("a", 65) }), "", ansBadJob},
		{"no endpoint", good(func(m map[string]any) { delete(m, "endpoint") }), "x1", ansBadJob},
		{"kind x", good(func(m map[string]any) { m["kind"] = "x" }), "x1", ansBadJob},
		{"kind text", good(func(m map[string]any) { m["kind"] = "a plan waits" }), "x1", ansBadJob},
		{"no name", good(func(m map[string]any) { delete(m, "name") }), "x1", ansBadJob},
		{"no lead", good(func(m map[string]any) { delete(m, "lead") }), "x1", ansBadJob},
		{"lead string", good(func(m map[string]any) { m["lead"] = "yes" }), "x1", ansBadJob},
		{"name number", good(func(m map[string]any) { m["name"] = 2 }), "x1", ansBadJob},
		{"name too long", good(func(m map[string]any) { m["name"] = strings.Repeat("a", pushNameMax+1) }), "x1", ansBadJob},
		{"name markup", good(func(m map[string]any) { m["name"] = "<b>x</b>" }), "x1", ansBadJob},
		{"name space", good(func(m map[string]any) { m["name"] = "a b" }), "x1", ansBadJob},
		{"name unicode", good(func(m map[string]any) { m["name"] = "é" }), "x1", ansBadJob},
		{"name quote", good(func(m map[string]any) { m["name"] = `a"b` }), "x1", ansBadJob},
		{"badge float", good(func(m map[string]any) { m["badge"] = 1.5 }), "x1", ansBadJob},
		{"badge string", good(func(m map[string]any) { m["badge"] = "3" }), "x1", ansBadJob},
		{"no badge", good(func(m map[string]any) { delete(m, "badge") }), "x1", ansBadJob},
		{"ttl negative", good(func(m map[string]any) { m["ttl"] = -1 }), "x1", ansBadJob},
		{"no ttl", good(func(m map[string]any) { delete(m, "ttl") }), "x1", ansBadJob},
		{"p256dh short", good(func(m map[string]any) { m["p256dh"] = "BAAA" }), "x1", ansBadJob},
		{"p256dh not on curve", good(func(m map[string]any) { m["p256dh"] = b64.EncodeToString(append([]byte{4}, make([]byte, 64)...)) }), "x1", ansBadJob},
		{"auth 15 bytes", good(func(m map[string]any) { m["auth"] = b64.EncodeToString(make([]byte, 15)) }), "x1", ansBadJob},
		{"endpoint refused", good(func(m map[string]any) { m["endpoint"] = "https://evil.example/abc" }), "x1", ansRefused},
		{"line over 4 KiB", good(func(m map[string]any) { m["id"] = "big"; m["pad"] = strings.Repeat("a", 5000) }), "", ansBadJob},
	}
	var in bytes.Buffer
	in.Write(configLine(t, cfg))
	for _, c := range cases {
		in.WriteString(c.line + "\n")
	}
	in.WriteString("\n   \n") // blank lines are skipped, not answered
	var out, logs bytes.Buffer
	servePush(context.Background(), &in, &out, func(f string, a ...any) { fmt.Fprintf(&logs, f+"\n", a...) }, nil)
	answers := parseAnswers(t, out.String())
	if len(answers) != len(cases) {
		t.Fatalf("%d answers for %d lines:\n%s", len(answers), len(cases), out.String())
	}
	for i, c := range cases {
		if answers[i].ID != c.id || answers[i].Status != c.status {
			t.Errorf("%s: %+v, want id %q status %q", c.name, answers[i], c.id, c.status)
		}
	}
}

func TestPushBadConfigAnswersEveryJob(t *testing.T) {
	ua, auth := newUA(t)
	in := strings.NewReader("{\"vapid_private\":\"nope\",\"sub\":\"https://x.example\"}\n" +
		string(jobLine("q1", "https://web.push.apple.com/abc", ua, auth, "p", 1, 60)) + "\n")
	var out bytes.Buffer
	servePush(context.Background(), in, &out, func(string, ...any) {}, nil)
	a := parseAnswers(t, out.String())
	if len(a) != 2 || a[0] != (pushAnswer{"", ansBadConfig}) || a[1] != (pushAnswer{"q1", ansBadConfig}) {
		t.Errorf("answers %+v", a)
	}
}

type failWriter struct{ n atomic.Int32 }

func (w *failWriter) Write([]byte) (int, error) { w.n.Add(1); return 0, syscall.EPIPE }

func TestPushClosedPipes(t *testing.T) {
	cfg := testVapid(t)
	// Jobs pipe closed: servePush returns.
	done := make(chan struct{})
	pr, pw := io.Pipe()
	go func() {
		servePush(context.Background(), pr, io.Discard, func(string, ...any) {}, nil)
		close(done)
	}()
	pw.Write(configLine(t, cfg))
	pw.Close()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("servePush still runs after the jobs pipe closed")
	}
	// Answers pipe closed: it stops at the first failed write, reads no more.
	w := &failWriter{}
	in := bytes.NewBuffer(configLine(t, cfg))
	in.WriteString("bad\nbad\nbad\n")
	servePush(context.Background(), in, w, func(string, ...any) {}, nil)
	if w.n.Load() != 1 {
		t.Errorf("%d writes after a closed answers pipe", w.n.Load())
	}
	// Nothing at all on the jobs pipe.
	servePush(context.Background(), strings.NewReader(""), w, func(string, ...any) {}, nil)
}

// Logs carry a job's id, kind and status only: never the endpoint, the keys,
// the auth secret or the token.
func TestPushLogsNoSecrets(t *testing.T) {
	var auths []string
	f := newFakeApple(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		auths = append(auths, r.Header.Get("Authorization"))
		w.WriteHeader(201)
	}))
	cfg := testVapid(t)
	ua, auth := newUA(t)
	endpoint := "https://web.push.apple.com/SecretPathToken123"
	var in bytes.Buffer
	cl := configLine(t, cfg)
	in.Write(cl)
	in.Write(append(jobLine("ok1", endpoint, ua, auth, "p", 2, 60), '\n'))
	in.Write(append(jobLine("rf1", "https://evil.example/SecretPathToken123", ua, auth, "n", 2, 60), '\n'))
	in.Write(append(jobLine("bad1", endpoint, ua, auth, "z", 2, 60), '\n'))
	var out, logs bytes.Buffer
	servePush(context.Background(), &in, &out, func(f string, a ...any) { fmt.Fprintf(&logs, f+"\n", a...) }, f.hooked)
	logText := logs.String()
	var cfgFields map[string]string
	json.Unmarshal(cl, &cfgFields)
	secrets := []string{"SecretPathToken123", "push.apple.com", "evil.example", cfgFields["vapid_private"], cfg.pubB64,
		b64.EncodeToString(ua.PublicKey().Bytes()), b64.EncodeToString(auth), "https"}
	if len(auths) != 1 {
		t.Fatalf("%d requests", len(auths))
	}
	secrets = append(secrets, strings.TrimPrefix(strings.Split(auths[0], ",")[0], "vapid t="))
	for _, s := range secrets {
		if strings.Contains(logText, s) || strings.Contains(out.String(), s) {
			t.Errorf("log or answers contain %q:\n%s", s, logText)
		}
	}
	for _, want := range []string{"push: ready", "push: job ok1 kind p status 201", "push: job rf1 kind - status refused", "push: job bad1 kind - status bad_job", "push: jobs pipe closed"} {
		if !strings.Contains(logText, want) {
			t.Errorf("log lacks %q:\n%s", want, logText)
		}
	}
}

// No new module: push.go imports the standard library only, and go.mod
// requires exactly today's modules.
func TestPushNoNewModule(t *testing.T) {
	f, err := parser.ParseFile(token.NewFileSet(), "push.go", nil, parser.ImportsOnly)
	if err != nil {
		t.Fatal(err)
	}
	for _, imp := range f.Imports {
		p := strings.Trim(imp.Path.Value, `"`)
		if first, _, _ := strings.Cut(p, "/"); strings.Contains(first, ".") {
			t.Errorf("push.go imports %s, not in the standard library", p)
		}
	}
	b, err := os.ReadFile("go.mod")
	if err != nil {
		t.Fatal(err)
	}
	var mods []string
	in := false
	for _, l := range strings.Split(string(b), "\n") {
		l = strings.TrimSpace(l)
		switch {
		case strings.HasPrefix(l, "require ("):
			in = true
		case in && l == ")":
			in = false
		case strings.HasPrefix(l, "require "):
			mods = append(mods, strings.Fields(strings.TrimPrefix(l, "require "))[0])
		case in && l != "":
			mods = append(mods, strings.Fields(l)[0])
		}
	}
	want := strings.Fields(`tailscale.com filippo.io/edwards25519 github.com/akutz/memconn
		github.com/alexbrainman/sspi github.com/coder/websocket github.com/creachadair/msync
		github.com/dblohm7/wingoes github.com/fxamacker/cbor/v2 github.com/gaissmai/bart
		github.com/go-json-experiment/json github.com/godbus/dbus/v5 github.com/golang/groupcache
		github.com/google/btree github.com/google/go-cmp github.com/hdevalence/ed25519consensus
		github.com/huin/goupnp github.com/jsimonetti/rtnetlink github.com/klauspost/compress
		github.com/mdlayher/netlink github.com/mdlayher/socket github.com/mitchellh/go-ps
		github.com/pires/go-proxyproto github.com/safchain/ethtool github.com/tailscale/certstore
		github.com/tailscale/go-winio github.com/tailscale/hujson github.com/tailscale/peercred
		github.com/tailscale/web-client-prebuilt github.com/tailscale/wireguard-go github.com/x448/float16
		go4.org/mem go4.org/netipx golang.org/x/crypto golang.org/x/exp golang.org/x/net
		golang.org/x/oauth2 golang.org/x/sync golang.org/x/sys golang.org/x/term golang.org/x/text
		golang.org/x/time golang.zx2c4.com/wintun golang.zx2c4.com/wireguard/windows gvisor.dev/gvisor`)
	sort.Strings(mods)
	sort.Strings(want)
	if strings.Join(mods, " ") != strings.Join(want, " ") {
		t.Errorf("go.mod modules changed:\n got %v\nwant %v", mods, want)
	}
}

// The binary: with --push it answers on stdout, survives a closed answers
// pipe, and keeps running when the jobs pipe closes.
func TestBinaryPushPipes(t *testing.T) {
	bin := binary(t)
	l := newLayout(t)
	cmd := exec.Command(bin, "--push",
		"--state-dir", l.stateDir, "--status-file", l.statusFile,
		"--parent-pid", strconv.Itoa(os.Getpid()),
		"--test-control-url", "http://"+closedPort(t), "--login-wait", "2s")
	cmd.Env = cleanEnv()
	var stderr strings.Builder
	cmd.Stderr = &stderr
	stdin, _ := cmd.StdinPipe()
	stdout, _ := cmd.StdoutPipe()
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()
	defer cmd.Process.Kill()

	br := bufio.NewReader(stdout)
	key := strings.Repeat("A", 42) + "E" // a 32-byte scalar of value 1, base64url
	io.WriteString(stdin, `{"vapid_private":"`+key+`","sub":"https://github.com/gallettom/sbxw"}`+"\n")
	io.WriteString(stdin, "not a job\n")
	line, err := br.ReadString('\n')
	if err != nil || strings.TrimSpace(line) != `{"id":"","status":"bad_job"}` {
		t.Fatalf("answer %q %v; stderr:\n%s", line, err, stderr.String())
	}
	stdout.Close() // the daemon stops reading answers
	io.WriteString(stdin, "still not a job\n")
	time.Sleep(500 * time.Millisecond)
	stdin.Close()
	time.Sleep(500 * time.Millisecond)
	select {
	case err := <-done:
		t.Fatalf("exited after the pipes closed: %v; stderr:\n%s", err, stderr.String())
	default:
	}
	cmd.Process.Signal(syscall.SIGTERM)
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("exit: %v; stderr:\n%s", err, stderr.String())
		}
	case <-time.After(20 * time.Second):
		t.Fatal("did not stop within 20 s of SIGTERM")
	}
	for _, want := range []string{"push: ready", "push: answers pipe closed"} {
		if !strings.Contains(stderr.String(), want) {
			t.Errorf("stderr lacks %q:\n%s", want, stderr.String())
		}
	}
	if strings.Contains(stderr.String(), key) {
		t.Error("stderr shows the key")
	}
}

// In push mode stdout carries answer lines and nothing else: tsnet's and Go's
// own logging go to stderr, while the node tries a control plane that does
// not answer for a few seconds.
func TestBinaryPushStdoutOnlyAnswers(t *testing.T) {
	bin := binary(t)
	l := newLayout(t)
	cmd := exec.Command(bin, "--push",
		"--state-dir", l.stateDir, "--status-file", l.statusFile,
		"--parent-pid", strconv.Itoa(os.Getpid()),
		"--test-control-url", "http://"+closedPort(t), "--login-wait", "1s")
	cmd.Env = cleanEnv()
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	stdin, _ := cmd.StdinPipe()
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()
	defer cmd.Process.Kill()

	cfg := testVapid(t)
	ua, auth := newUA(t)
	stdin.Write(configLine(t, cfg))
	jobs := []struct{ line, id, status string }{
		{"garbage", "", ansBadJob},
		{string(jobLine("r1", "https://evil.example/abc", ua, auth, "p", 1, 60)), "r1", ansRefused},
		{string(jobLine("k1", "https://web.push.apple.com/abc", ua, auth, "x", 1, 60)), "k1", ansBadJob},
		{strings.Repeat("z", 5000), "", ansBadJob},
		{string(jobLine("r2", "https://[::1]/abc", ua, auth, "n", 1, 60)), "r2", ansRefused},
	}
	for _, j := range jobs {
		io.WriteString(stdin, j.line+"\n")
		time.Sleep(700 * time.Millisecond) // let the node log in between
	}
	time.Sleep(2 * time.Second)
	stdin.Close()
	cmd.Process.Signal(syscall.SIGTERM)
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("exit: %v; stderr:\n%s", err, stderr.String())
		}
	case <-time.After(20 * time.Second):
		t.Fatal("did not stop within 20 s of SIGTERM")
	}
	if !strings.HasSuffix(stdout.String(), "\n") {
		t.Errorf("stdout does not end with a newline: %q", stdout.String())
	}
	answers := parseAnswers(t, stdout.String()) // fails on any line that is not an answer
	if len(answers) != len(jobs) {
		t.Fatalf("%d stdout lines for %d jobs:\n%s", len(answers), len(jobs), stdout.String())
	}
	for i, j := range jobs {
		if answers[i] != (pushAnswer{j.id, j.status}) {
			t.Errorf("answer %d: %+v, want %q %q", i, answers[i], j.id, j.status)
		}
	}
	if !strings.Contains(stderr.String(), "state ") || !strings.Contains(stderr.String(), "push: ready") {
		t.Errorf("stderr lacks the state and push lines:\n%s", stderr.String())
	}
}
