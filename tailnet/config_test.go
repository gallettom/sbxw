package main

import (
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestForwardIsLoopbackOnly(t *testing.T) {
	ok := map[string]string{
		"127.0.0.1:7683":        "127.0.0.1:7683",
		"127.8.9.10:9000":       "127.8.9.10:9000",
		"[::1]:7683":            "[::1]:7683",
		"[::ffff:127.0.0.1]:80": "127.0.0.1:80",
	}
	for in, want := range ok {
		got, err := checkForward(in)
		if err != nil || got != want {
			t.Errorf("%q: got %q, %v", in, got, err)
		}
	}
	for _, in := range []string{
		"", "localhost:7683", "sbxw.localhost:7683", "0.0.0.0:7683", "[::]:7683",
		"10.0.0.1:7683", "100.64.0.1:7683", "192.168.1.2:7683", "8.8.8.8:443",
		"127.0.0.1", "127.0.0.1:0", "127.0.0.1:7681", "127.0.0.1:7682", "[::1]:7681",
		"http://127.0.0.1:7683", "127.0.0.1:7683/x",
	} {
		if _, err := checkForward(in); err == nil {
			t.Errorf("%q accepted", in)
		}
	}
}

func TestStateDirRule(t *testing.T) {
	for _, in := range []string{"/Users/a/.sbxw/remote/tailnet", "/tmp/x/remote/tailnet"} {
		if _, err := checkStateDir(in); err != nil {
			t.Errorf("%q: %v", in, err)
		}
	}
	for _, in := range []string{
		"", "remote/tailnet", "/", "/Users/a", "/Users/a/.sbxw", "/Users/a/.sbxw/remote",
		"/Users/a/.sbxw/remote/tailnet/", "/Users/a/.sbxw/remote/../remote/tailnet", "/x/tailnet",
	} {
		if _, err := checkStateDir(in); err == nil {
			t.Errorf("%q accepted", in)
		}
	}
	if _, err := checkStatusFile("/a/remote/tailnet/s.json", "/a/remote/tailnet"); err == nil {
		t.Error("status file inside the state dir accepted")
	}
}

func TestHostnameAndControlURL(t *testing.T) {
	for _, in := range []string{"sbxw", "my-mac", "a"} {
		if _, err := checkHostname(in); err != nil {
			t.Errorf("%q: %v", in, err)
		}
	}
	for _, in := range []string{"", "SBXW", "-x", "x-", "a.b", "a b", strings.Repeat("a", 64)} {
		if _, err := checkHostname(in); err == nil {
			t.Errorf("hostname %q accepted", in)
		}
	}
	for _, in := range []string{"https://controlplane.tailscale.com", "http://10.0.0.1:1", "ftp://127.0.0.1", "http://localhost:1"} {
		if _, err := checkControlURL(in); err == nil {
			t.Errorf("control URL %q accepted", in)
		}
	}
	if _, err := checkControlURL("http://127.0.0.1:9"); err != nil {
		t.Error(err)
	}
}

func TestParseRun(t *testing.T) {
	base := []string{"--state-dir", "/h/remote/tailnet", "--status-file", "/h/remote/tailnet.json", "--parent-pid", "4242"}
	c, err := parseRun(base, io.Discard)
	if err != nil {
		t.Fatal(err)
	}
	if c.forward != "127.0.0.1:7683" || c.hostname != "sbxw" || c.parentPID != 4242 {
		t.Fatalf("defaults: %+v", c)
	}
	bad := [][]string{
		append([]string{"--forward", "0.0.0.0:7683"}, base...),
		append([]string{"--forward", "127.0.0.1:7681"}, base...),
		{"--state-dir", "/h/remote/tailnet", "--status-file", "/h/remote/tailnet.json"}, // no parent
		append(append([]string{}, base...), "extra"),
		append([]string{"--auth-key", "tskey-x"}, base...),
	}
	for _, args := range bad {
		if _, err := parseRun(args, io.Discard); err == nil {
			t.Errorf("accepted %q", args)
		}
	}
}

func TestCheckEnv(t *testing.T) {
	if err := checkEnv([]string{"HOME=/h", "PATH=/bin"}); err != nil {
		t.Fatal(err)
	}
	for _, kv := range []string{"TS_AUTHKEY=tskey", "TSNET_FORCE_LOGIN=1", "ts_debug=1", "TS_CONTROL_URL=x"} {
		err := checkEnv([]string{"HOME=/h", kv})
		if err == nil {
			t.Errorf("%q accepted", kv)
		} else if strings.Contains(err.Error(), "tskey") {
			t.Errorf("error leaks the value: %v", err)
		}
	}
}

func TestStatusFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "tailnet.json")
	w := newStatusWriter(path)
	w.now = func() time.Time { return time.UnixMilli(1763650000000) }
	var changes []string
	w.onChange = func(s string) { changes = append(changes, s) }

	read := func() map[string]any {
		t.Helper()
		b, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		var m map[string]any
		if err := json.Unmarshal(b, &m); err != nil {
			t.Fatal(err)
		}
		return m
	}

	if err := w.set(stateNeedsLogin, "ignored", "https://login.tailscale.com/a/xyz", "Open the link."); err != nil {
		t.Fatal(err)
	}
	m := read()
	if len(m) != 6 {
		t.Fatalf("fields %v", m)
	}
	for _, k := range []string{"state", "name", "login_url", "detail", "pid", "updated_ms"} {
		if _, ok := m[k]; !ok {
			t.Errorf("missing %s", k)
		}
	}
	if m["state"] != "needs_login" || m["login_url"] != "https://login.tailscale.com/a/xyz" || m["name"] != nil {
		t.Fatalf("got %v", m)
	}
	if m["updated_ms"].(float64) != 1763650000000 || int(m["pid"].(float64)) != os.Getpid() {
		t.Fatalf("got %v", m)
	}
	fi, _ := os.Stat(path)
	if fi.Mode().Perm() != 0o600 {
		t.Fatalf("mode %v", fi.Mode())
	}

	// The login URL never survives a state change.
	w.set(stateRunning, testName, "https://login.tailscale.com/a/xyz", "")
	m = read()
	if m["state"] != "running" || m["name"] != testName || m["login_url"] != nil || m["detail"] != nil {
		t.Fatalf("got %v", m)
	}

	w.set(stateRunning, testName, "", "")
	if strings.Join(changes, ",") != "needs_login,running" {
		t.Fatalf("changes %v", changes)
	}

	// Heartbeat rewrites updated_ms.
	w.now = func() time.Time { return time.UnixMilli(1763650005000) }
	w.beat()
	if read()["updated_ms"].(float64) != 1763650005000 {
		t.Fatal("no heartbeat")
	}

	// A huge detail is dropped, the file stays under the cap.
	w.set(stateError, "", "", strings.Repeat("x", 20000))
	b, _ := os.ReadFile(path)
	if len(b) > maxStatusBytes {
		t.Fatalf("status file %d bytes", len(b))
	}

	if err := w.remove(); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatal("not removed")
	}
	entries, _ := os.ReadDir(dir)
	if len(entries) != 0 {
		t.Fatalf("left behind: %v", entries)
	}
}

func TestNames(t *testing.T) {
	for _, s := range []string{testName, "a", "a-b.c"} {
		if !validTailnetName(s) {
			t.Errorf("%q refused", s)
		}
	}
	for _, s := range []string{"", "a..b", ".a", "A.b", "a_b", "a b", "a/b", strings.Repeat("a", 64) + ".b"} {
		if validTailnetName(s) {
			t.Errorf("%q accepted", s)
		}
	}
	for _, s := range []string{"https://login.tailscale.com/a/1"} {
		if !safeLoginURL(s) {
			t.Errorf("%q refused", s)
		}
	}
	for _, s := range []string{"", "http://login.tailscale.com/a", "javascript:alert(1)", "https://u:p@x/a", "https:///a"} {
		if safeLoginURL(s) {
			t.Errorf("%q accepted", s)
		}
	}
}
