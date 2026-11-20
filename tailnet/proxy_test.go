package main

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"
)

const testName = "sbxw.tail1234.ts.net"

// seen records what the upstream received.
type seen struct {
	mu     sync.Mutex
	host   string
	path   string
	query  string
	header http.Header
	body   int
}

func newUpstream(t *testing.T, s *seen) *httptest.Server {
	t.Helper()
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		n, _ := io.Copy(io.Discard, r.Body)
		s.mu.Lock()
		s.host, s.path, s.query, s.header, s.body = r.Host, r.URL.EscapedPath(), r.URL.RawQuery, r.Header.Clone(), int(n)
		s.mu.Unlock()
		if r.URL.Path == "/r/events" {
			w.Header().Set("Content-Type", "text/event-stream")
			w.WriteHeader(200)
			fmt.Fprint(w, "event: one\ndata: 1\n\n")
			w.(http.Flusher).Flush()
			<-r.Context().Done() // hold the stream open
			return
		}
		w.WriteHeader(200)
		io.WriteString(w, "ok")
	}))
	t.Cleanup(up.Close)
	return up
}

func fakeWhoIs(_ string, err error) whoIsFunc {
	return func(context.Context, string) error { return err }
}

// newFront serves the proxy on a plain local listener standing in for the
// tailnet listener (no real tailnet in tests).
func newFront(t *testing.T, upstream string, who whoIsFunc) *httptest.Server {
	t.Helper()
	return newFrontWith(t, newProxy(upstream, func() string { return testName }, who))
}

func newFrontWith(t *testing.T, px *proxy) *httptest.Server {
	t.Helper()
	front := httptest.NewUnstartedServer(px)
	front.Config = newHTTPServer(px)
	front.Start()
	t.Cleanup(front.Close)
	return front
}

func do(t *testing.T, front *httptest.Server, req *http.Request) *http.Response {
	t.Helper()
	req.URL.Scheme = "http"
	req.URL.Host = strings.TrimPrefix(front.URL, "http://")
	res, err := http.DefaultTransport.RoundTrip(req)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { res.Body.Close() })
	return res
}

func get(t *testing.T, path string) *http.Request {
	req, err := http.NewRequest("GET", "http://placeholder"+path, nil)
	if err != nil {
		t.Fatal(err)
	}
	req.Host = testName
	return req
}

func TestStripAndSetHeaders(t *testing.T) {
	var s seen
	up := newUpstream(t, &s)
	front := newFront(t, strings.TrimPrefix(up.URL, "http://"), fakeWhoIs("phone.tail1234.ts.net", nil))

	req := get(t, "/r/me?x=1")
	for _, h := range []string{
		"X-Forwarded-For", "X-Forwarded-Host", "X-Forwarded-Proto", "X-Forwarded-Anything",
		"Forwarded", "X-Real-Ip", "X-Sbxw-Tailnet-Node", "X-Sbxw-Tailnet-User", "X-Sbxw-Other",
		"Tailscale-User-Login", "Tailscale-Whatever", "X-Custom-Hop",
	} {
		req.Header.Set(h, "evil")
	}
	req.Header.Set("Connection", "X-Custom-Hop")
	req.Header.Set("Cookie", "__Host-sbxw_device=abc")
	req.Header.Set("Origin", "https://"+testName)
	res := do(t, front, req)
	if res.StatusCode != 200 {
		t.Fatalf("status %d", res.StatusCode)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	for name, vals := range s.header {
		for _, v := range vals {
			if strings.Contains(v, "evil") {
				t.Errorf("client header reached upstream: %s: %s", name, v)
			}
		}
	}
	if got := s.header.Values("X-Forwarded-For"); len(got) != 1 || got[0] != "127.0.0.1" {
		t.Errorf("X-Forwarded-For = %q, want the peer IP only", got)
	}
	for name := range s.header {
		if strings.HasPrefix(strings.ToLower(name), "x-sbxw-") || strings.HasPrefix(strings.ToLower(name), "tailscale-") {
			t.Errorf("identity header reached upstream: %s", name)
		}
	}
	if s.header.Get("Cookie") != "__Host-sbxw_device=abc" || s.header.Get("Origin") != "https://"+testName {
		t.Error("cookie or origin not passed through")
	}
	if s.host != testName {
		t.Errorf("Host = %q, want %q", s.host, testName)
	}
	if s.path != "/r/me" || s.query != "x=1" {
		t.Errorf("path rewritten: %q ? %q", s.path, s.query)
	}
}

func TestNoPathRewriting(t *testing.T) {
	var s seen
	up := newUpstream(t, &s)
	front := newFront(t, strings.TrimPrefix(up.URL, "http://"), fakeWhoIs("p.ts.net", nil))
	for _, p := range []string{"/r/%2E%2E/api/x", "/R/me", "/r/a%2Fb"} {
		res := do(t, front, get(t, p))
		if res.StatusCode != 200 {
			t.Fatalf("%s: status %d", p, res.StatusCode)
		}
		s.mu.Lock()
		got := s.path
		s.mu.Unlock()
		if got != p {
			t.Errorf("path %q reached upstream as %q", p, got)
		}
	}
}

func TestHostMustBeTheTailnetName(t *testing.T) {
	var s seen
	up := newUpstream(t, &s)
	front := newFront(t, strings.TrimPrefix(up.URL, "http://"), fakeWhoIs("p.ts.net", nil))
	for host, want := range map[string]int{
		testName:                  200,
		testName + ":443":         200,
		strings.ToUpper(testName): 200,
		"evil.example":            421,
		"127.0.0.1:7683":          421,
		testName + ":8443":        421,
		"sbxw.localhost":          421,
	} {
		req := get(t, "/")
		req.Host = host
		if res := do(t, front, req); res.StatusCode != want {
			t.Errorf("Host %q: status %d, want %d", host, res.StatusCode, want)
		}
	}
}

func TestNotRunningRefusesEverything(t *testing.T) {
	px := newProxy("127.0.0.1:1", func() string { return "" }, fakeWhoIs("p.ts.net", nil))
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("GET", "/", nil)
	req.Host = ""
	px.ServeHTTP(rec, req)
	if rec.Code != 421 {
		t.Fatalf("status %d", rec.Code)
	}
}

func TestUnknownPeerRefused(t *testing.T) {
	var s seen
	up := newUpstream(t, &s)
	front := newFront(t, strings.TrimPrefix(up.URL, "http://"), fakeWhoIs("", errors.New("no such peer")))
	if res := do(t, front, get(t, "/")); res.StatusCode != 403 {
		t.Fatalf("status %d", res.StatusCode)
	}
	if s.host != "" {
		t.Fatal("request reached upstream")
	}
}

func TestUpgradeRefused(t *testing.T) {
	var s seen
	up := newUpstream(t, &s)
	front := newFront(t, strings.TrimPrefix(up.URL, "http://"), fakeWhoIs("p.ts.net", nil))
	req := get(t, "/ws")
	req.Header.Set("Connection", "Upgrade")
	req.Header.Set("Upgrade", "websocket")
	if res := do(t, front, req); res.StatusCode != 400 {
		t.Fatalf("status %d", res.StatusCode)
	}
	if s.host != "" {
		t.Fatal("upgrade reached upstream")
	}
}

func TestBodyLimit(t *testing.T) {
	var s seen
	up := newUpstream(t, &s)
	front := newFront(t, strings.TrimPrefix(up.URL, "http://"), fakeWhoIs("p.ts.net", nil))

	// Declared too large: refused before anything is forwarded.
	req, _ := http.NewRequest("POST", "http://x/r/voice/transcribe", strings.NewReader(strings.Repeat("a", maxBodyBytes+1)))
	req.Host = testName
	if res := do(t, front, req); res.StatusCode != 413 {
		t.Fatalf("declared length: status %d", res.StatusCode)
	}

	// Chunked and too large: cut at the cap, the upstream never gets more.
	req, _ = http.NewRequest("POST", "http://x/r/voice/transcribe", io.NopCloser(strings.NewReader(strings.Repeat("a", maxBodyBytes+4096))))
	req.Host = testName
	req.ContentLength = -1
	res := do(t, front, req)
	s.mu.Lock()
	got := s.body
	s.mu.Unlock()
	if res.StatusCode == 200 && got > maxBodyBytes {
		t.Fatalf("upstream received %d bytes", got)
	}

	// At the daemon's transcribe cap: passes.
	req, _ = http.NewRequest("POST", "http://x/r/voice/transcribe", strings.NewReader(strings.Repeat("a", 7681024)))
	req.Host = testName
	if res := do(t, front, req); res.StatusCode != 200 {
		t.Fatalf("transcribe-sized body: status %d", res.StatusCode)
	}
}

func TestHeaderLimit(t *testing.T) {
	var s seen
	up := newUpstream(t, &s)
	front := newFront(t, strings.TrimPrefix(up.URL, "http://"), fakeWhoIs("p.ts.net", nil))
	req := get(t, "/")
	req.Header.Set("X-Big", strings.Repeat("a", maxHeaderBytes+8192))
	if res := do(t, front, req); res.StatusCode != http.StatusRequestHeaderFieldsTooLarge {
		t.Fatalf("status %d", res.StatusCode)
	}
}

func TestEventStreamIsNotBuffered(t *testing.T) {
	var s seen
	up := newUpstream(t, &s)
	front := newFront(t, strings.TrimPrefix(up.URL, "http://"), fakeWhoIs("p.ts.net", nil))
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	req := get(t, "/r/events").WithContext(ctx)
	res := do(t, front, req)
	line, err := bufio.NewReader(res.Body).ReadString('\n')
	if err != nil {
		t.Fatalf("no event before the stream ended: %v", err)
	}
	if line != "event: one\n" {
		t.Fatalf("got %q", line)
	}
}

func TestOnlyTheUpstreamIsDialled(t *testing.T) {
	var s seen
	up := newUpstream(t, &s)
	front := newFront(t, strings.TrimPrefix(up.URL, "http://"), fakeWhoIs("p.ts.net", nil))
	// An absolute-form request naming another host still goes to the upstream.
	conn, err := net.Dial("tcp", strings.TrimPrefix(front.URL, "http://"))
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	fmt.Fprintf(conn, "GET http://127.0.0.1:7681/api/x HTTP/1.1\r\nHost: %s\r\nConnection: close\r\n\r\n", testName)
	res, err := http.ReadResponse(bufio.NewReader(conn), nil)
	if err != nil {
		t.Fatal(err)
	}
	res.Body.Close()
	s.mu.Lock()
	defer s.mu.Unlock()
	if res.StatusCode == 200 && s.path != "/api/x" {
		t.Fatalf("status %d path %q", res.StatusCode, s.path)
	}
}

func TestUpstreamDownIsA502WithoutDetail(t *testing.T) {
	front := newFront(t, "127.0.0.1:1", fakeWhoIs("p.ts.net", nil))
	res := do(t, front, get(t, "/r/me?secret=1"))
	b, _ := io.ReadAll(res.Body)
	if res.StatusCode != 502 || strings.Contains(string(b), "secret") || strings.Contains(string(b), "127.0.0.1") {
		t.Fatalf("status %d body %q", res.StatusCode, b)
	}
}

func TestStripHeadersUnit(t *testing.T) {
	h := http.Header{}
	h.Set("x-forwarded-for", "a")
	h["x-sbxw-raw"] = []string{"b"} // non-canonical key
	h.Set("Tailscale-Funnel-Request", "c")
	h.Set("Keep", "d")
	stripHeaders(h)
	if len(h) != 1 || h.Get("Keep") != "d" {
		t.Fatalf("left %v", h)
	}
}
