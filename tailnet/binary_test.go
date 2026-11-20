package main

import (
	"encoding/json"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"
)

// These tests build the real binary and start it against a control plane
// that does not answer (a closed loopback port). No real tailnet is used.

var (
	buildOnce sync.Once
	binPath   string
	buildErr  error
)

func binary(t *testing.T) string {
	t.Helper()
	if testing.Short() {
		t.Skip("builds the binary")
	}
	buildOnce.Do(func() {
		dir, err := os.MkdirTemp("", "sbxw-tailnet-test-")
		if err != nil {
			buildErr = err
			return
		}
		binPath = filepath.Join(dir, "sbxw-tailnet")
		cmd := exec.Command("go", "build", "-trimpath", "-tags", buildTags(), "-o", binPath, ".")
		cmd.Env = append(os.Environ(), "CGO_ENABLED=0")
		out, err := cmd.CombinedOutput()
		if err != nil {
			buildErr = err
			binPath = string(out)
		}
	})
	if buildErr != nil {
		t.Fatalf("build: %v\n%s", buildErr, binPath)
	}
	return binPath
}

// buildTags reads the same tag list as scripts/build-tailnet.sh.
func buildTags() string {
	b, err := os.ReadFile("build-tags.txt")
	if err != nil {
		return ""
	}
	var tags []string
	for _, line := range strings.Split(string(b), "\n") {
		line = strings.TrimSpace(line)
		if line != "" && !strings.HasPrefix(line, "#") {
			tags = append(tags, line)
		}
	}
	return strings.Join(tags, ",")
}

func closedPort(t *testing.T) string {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	addr := l.Addr().String()
	l.Close()
	return addr
}

func cleanEnv() []string {
	return []string{"HOME=" + os.TempDir(), "TMPDIR=" + os.TempDir()}
}

type layout struct{ stateDir, statusFile string }

func newLayout(t *testing.T) layout {
	root := filepath.Join(t.TempDir(), "remote")
	return layout{filepath.Join(root, "tailnet"), filepath.Join(root, "tailnet.json")}
}

func readStatus(path string) (status, bool) {
	b, err := os.ReadFile(path)
	if err != nil {
		return status{}, false
	}
	var s status
	if json.Unmarshal(b, &s) != nil {
		return status{}, false
	}
	return s, true
}

func TestBinaryUnreachableControlThenStop(t *testing.T) {
	bin := binary(t)
	l := newLayout(t)
	cmd := exec.Command(bin,
		"--forward", "127.0.0.1:7683",
		"--state-dir", l.stateDir, "--status-file", l.statusFile,
		"--parent-pid", strconv.Itoa(os.Getpid()),
		"--test-control-url", "http://"+closedPort(t),
		"--login-wait", "2s")
	cmd.Env = cleanEnv()
	var stderr strings.Builder
	cmd.Stderr = &stderr
	var stdout strings.Builder
	cmd.Stdout = &stdout
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()

	deadline := time.Now().Add(60 * time.Second)
	var last status
	for time.Now().Before(deadline) {
		if s, ok := readStatus(l.statusFile); ok {
			last = s
			if s.State == stateNeedsLogin || s.State == stateError {
				break
			}
		}
		time.Sleep(200 * time.Millisecond)
	}
	if last.State != stateNeedsLogin && last.State != stateError {
		cmd.Process.Kill()
		t.Fatalf("state %q after 60 s; stderr:\n%s", last.State, stderr.String())
	}
	if last.PID != cmd.Process.Pid {
		t.Errorf("pid %d, want %d", last.PID, cmd.Process.Pid)
	}
	fi, err := os.Stat(l.stateDir)
	if err != nil || fi.Mode().Perm() != 0o700 {
		t.Errorf("state dir: %v %v", fi, err)
	}

	cmd.Process.Signal(syscall.SIGTERM)
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("exit: %v; stderr:\n%s", err, stderr.String())
		}
	case <-time.After(20 * time.Second):
		cmd.Process.Kill()
		t.Fatal("did not stop within 20 s of SIGTERM")
	}
	if _, err := os.Stat(l.statusFile); !os.IsNotExist(err) {
		t.Error("status file left after a clean stop")
	}
	if stdout.Len() != 0 {
		t.Errorf("stdout not empty: %q", stdout.String())
	}
	if strings.Contains(stderr.String(), "http") {
		t.Errorf("stderr names a URL:\n%s", stderr.String())
	}
}

func TestBinaryExitsWhenParentIsNotItsParent(t *testing.T) {
	bin := binary(t)
	l := newLayout(t)
	cmd := exec.Command(bin,
		"--state-dir", l.stateDir, "--status-file", l.statusFile,
		"--parent-pid", "999999",
		"--test-control-url", "http://"+closedPort(t))
	cmd.Env = cleanEnv()
	start := time.Now()
	done := make(chan error, 1)
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	go func() { done <- cmd.Wait() }()
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("exit: %v", err)
		}
	case <-time.After(20 * time.Second):
		cmd.Process.Kill()
		t.Fatal("still running 20 s after its parent was gone")
	}
	t.Logf("stopped after %v", time.Since(start))
}

func TestBinaryRefusesBadStart(t *testing.T) {
	bin := binary(t)
	l := newLayout(t)
	args := []string{"--state-dir", l.stateDir, "--status-file", l.statusFile, "--parent-pid", strconv.Itoa(os.Getpid())}
	cases := []struct {
		args []string
		env  []string
	}{
		{append([]string{"--forward", "0.0.0.0:7683"}, args...), cleanEnv()},
		{append([]string{"--forward", "127.0.0.1:7681"}, args...), cleanEnv()},
		{args, append(cleanEnv(), "TS_AUTHKEY=tskey-auth-xyz")},
		{[]string{"forget", "--state-dir", "/tmp", "--status-file", l.statusFile}, cleanEnv()},
	}
	for _, c := range cases {
		cmd := exec.Command(bin, c.args...)
		cmd.Env = c.env
		out, err := cmd.CombinedOutput()
		ee, ok := err.(*exec.ExitError)
		if !ok || ee.ExitCode() != 2 {
			t.Errorf("%q: %v, want exit 2", c.args, err)
		}
		if strings.Contains(string(out), "tskey") {
			t.Errorf("output leaks the key: %s", out)
		}
	}
	if _, err := os.Stat(l.statusFile); !os.IsNotExist(err) {
		t.Error("a refused start wrote a status file")
	}
}

func TestBinaryForgetDeletesState(t *testing.T) {
	bin := binary(t)
	l := newLayout(t)
	os.MkdirAll(l.stateDir, 0o700)
	os.WriteFile(filepath.Join(l.stateDir, "something"), []byte("x"), 0o600)
	os.WriteFile(l.statusFile, []byte("{}"), 0o600)
	cmd := exec.Command(bin, "forget", "--state-dir", l.stateDir, "--status-file", l.statusFile)
	cmd.Env = cleanEnv()
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%v: %s", err, out)
	}
	if _, err := os.Stat(l.stateDir); !os.IsNotExist(err) {
		t.Error("state dir left")
	}
	if _, err := os.Stat(l.statusFile); !os.IsNotExist(err) {
		t.Error("status file left")
	}
	if _, err := os.Stat(filepath.Dir(l.stateDir)); err != nil {
		t.Error("forget removed the parent directory")
	}
}
