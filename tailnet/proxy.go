package main

import (
	"context"
	"io"
	"log"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"strings"
	"sync"
	"time"
)

// Limits of the tailnet-facing HTTP server. The body cap covers the daemon's
// largest route (a phone's file upload: 25 MiB, API.md 2.6.1; before it, voice
// transcription: 7,681,024 bytes) with a margin. The body is streamed to the
// daemon, never held whole here; the daemon counts its own cap again.
const (
	maxHeaderBytes    = 16 << 10
	maxBodyBytes      = 26 << 20
	readHeaderTimeout = 10 * time.Second
	bodyReadTimeout   = 120 * time.Second
	idleTimeout       = 120 * time.Second
	upstreamDialTO    = 5 * time.Second
	upstreamHeaderTO  = 5 * time.Minute
)

// Headers set by this program. Whatever the client sent under these names, or
// under any of the stripped prefixes, never reaches the daemon.
const (
	hdrForwardedFor = "X-Forwarded-For"
)

// strippedPrefixes are removed from every incoming request, compared on the
// canonical (lower-cased here) header name.
var strippedPrefixes = []string{"x-forwarded-", "x-sbxw-", "tailscale-"}

// strippedExact are removed as well.
var strippedExact = []string{"forwarded", "x-real-ip", "x-client-ip", "true-client-ip"}

// hopByHop are connection-level headers that never cross a proxy.
var hopByHop = []string{
	"Connection", "Proxy-Connection", "Keep-Alive", "Proxy-Authenticate",
	"Proxy-Authorization", "Te", "Trailer", "Transfer-Encoding", "Upgrade",
}

// stripHeaders removes every header a client could use to pass itself off as
// this proxy or as Tailscale, and every hop-by-hop header (including those
// named in Connection).
func stripHeaders(h http.Header) {
	for _, v := range h.Values("Connection") {
		for _, name := range strings.Split(v, ",") {
			if name = strings.TrimSpace(name); name != "" {
				h.Del(name)
			}
		}
	}
	for _, name := range hopByHop {
		h.Del(name)
	}
	for name := range h {
		lower := strings.ToLower(name)
		drop := false
		for _, p := range strippedPrefixes {
			if strings.HasPrefix(lower, p) {
				drop = true
			}
		}
		for _, e := range strippedExact {
			if lower == e {
				drop = true
			}
		}
		if drop {
			delete(h, name)
		}
	}
}

type peerKey struct{}

type peerID struct{ ip string }

// whoIsFunc decides whether the peer behind a connection's remote address may
// connect: nil admits it. What it learns about the peer stays here: no
// identity is forwarded or logged.
type whoIsFunc func(ctx context.Context, remoteAddr string) error

// proxy forwards requests from the tailnet to exactly one loopback upstream.
type proxy struct {
	upstream string        // validated loopback IP:PORT
	name     func() string // the tailnet name of this node; "" until running
	whoIs    whoIsFunc
	rp       *httputil.ReverseProxy

	// onRefuse, if set, is told of a refused peer, at most once a minute,
	// without the peer's name or address.
	onRefuse    func()
	refuseMu    sync.Mutex
	lastRefused time.Time
}

func (p *proxy) noteRefused() {
	if p.onRefuse == nil {
		return
	}
	p.refuseMu.Lock()
	now := time.Now()
	due := p.lastRefused.IsZero() || now.Sub(p.lastRefused) >= time.Minute
	if due {
		p.lastRefused = now
	}
	p.refuseMu.Unlock()
	if due {
		p.onRefuse()
	}
}

func newProxy(upstream string, name func() string, whoIs whoIsFunc) *proxy {
	p := &proxy{upstream: upstream, name: name, whoIs: whoIs}
	target := &url.URL{Scheme: "http", Host: upstream}
	dialer := &net.Dialer{Timeout: upstreamDialTO}
	transport := &http.Transport{
		// Never an environment proxy: the upstream is local.
		Proxy: nil,
		// Whatever address the request names, dial only the upstream.
		DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
			return dialer.DialContext(ctx, "tcp", upstream)
		},
		ForceAttemptHTTP2:     false,
		DisableCompression:    true,
		MaxIdleConns:          8,
		IdleConnTimeout:       60 * time.Second,
		ResponseHeaderTimeout: upstreamHeaderTO,
	}
	p.rp = &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
			pr.SetURL(target)
			// SetURL clears Host; the daemon checks it, so send the tailnet name.
			pr.Out.Host = p.name()
			// Rewrite mode has already dropped X-Forwarded-*; set ours on the
			// outgoing request, from what ServeHTTP checked.
			if id, ok := pr.In.Context().Value(peerKey{}).(peerID); ok {
				pr.Out.Header.Set(hdrForwardedFor, id.ip)
			}
		},
		Transport: transport,
		// Flush at once: server-sent events must stream.
		FlushInterval: -1,
		// The default error log names the URL, path and query included.
		ErrorLog: log.New(io.Discard, "", 0),
		ErrorHandler: func(w http.ResponseWriter, _ *http.Request, _ error) {
			plain(w, http.StatusBadGateway, "sbxw is not answering.")
		},
	}
	return p
}

func plain(w http.ResponseWriter, code int, msg string) {
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.WriteHeader(code)
	_, _ = io.WriteString(w, msg+"\n")
}

// hostMatches reports whether a request's Host names this node: exactly the
// tailnet name, or the name with :443.
func hostMatches(host, name string) bool {
	if name == "" {
		return false
	}
	host = strings.ToLower(host)
	return host == name || host == name+":443"
}

func (p *proxy) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	name := p.name()
	if !hostMatches(r.Host, name) {
		plain(w, http.StatusMisdirectedRequest, "Wrong host name.")
		return
	}
	if r.Header.Get("Upgrade") != "" || r.Method == http.MethodConnect {
		plain(w, http.StatusBadRequest, "Not supported.")
		return
	}
	if r.ContentLength > maxBodyBytes {
		plain(w, http.StatusRequestEntityTooLarge, "Request too large.")
		return
	}
	if err := p.whoIs(r.Context(), r.RemoteAddr); err != nil {
		// Not one of the person's own devices: nothing to forward.
		p.noteRefused()
		plain(w, http.StatusForbidden, "Forbidden.")
		return
	}
	peerIP, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		plain(w, http.StatusBadRequest, "Bad peer address.")
		return
	}
	if r.Body != nil && r.Body != http.NoBody {
		r.Body = http.MaxBytesReader(w, r.Body, maxBodyBytes)
		_ = http.NewResponseController(w).SetReadDeadline(time.Now().Add(bodyReadTimeout))
	}

	stripHeaders(r.Header)
	ctx := context.WithValue(r.Context(), peerKey{}, peerID{ip: peerIP})
	p.rp.ServeHTTP(w, r.WithContext(ctx))
}

// newHTTPServer wraps the proxy with the server-side limits.
func newHTTPServer(h http.Handler) *http.Server {
	return &http.Server{
		Handler:           h,
		MaxHeaderBytes:    maxHeaderBytes,
		ReadHeaderTimeout: readHeaderTimeout,
		IdleTimeout:       idleTimeout,
		// No WriteTimeout: event streams stay open. No ReadTimeout: the body
		// deadline is set per request in ServeHTTP.
		ErrorLog: log.New(io.Discard, "", 0),
	}
}
