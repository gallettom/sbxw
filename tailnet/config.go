package main

import (
	"errors"
	"flag"
	"fmt"
	"io"
	"net/netip"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"
)

// Ports of the daemon's other listeners: the sidecar never forwards there,
// whatever it is told.
var refusedPorts = map[uint16]string{7681: "the UI listener", 7682: "the agent listener"}

type runConfig struct {
	forward    string
	stateDir   string
	statusFile string
	hostname   string
	parentPID  int
	controlURL string        // tests only: a loopback http URL
	loginWait  time.Duration // how long to wait for a login link before saying so
	push       bool          // read push jobs on stdin, answer on stdout (push.go)
}

type forgetConfig struct {
	stateDir   string
	statusFile string
	controlURL string
}

// checkForward accepts only a loopback IP:PORT literal (127.0.0.0/8 or ::1),
// never a host name, never the daemon's UI or agent port.
func checkForward(s string) (string, error) {
	ap, err := netip.ParseAddrPort(s)
	if err != nil {
		return "", fmt.Errorf("--forward must be a loopback IP:PORT, such as 127.0.0.1:7683")
	}
	if !ap.Addr().Unmap().IsLoopback() {
		return "", fmt.Errorf("--forward must be a loopback address (127.0.0.0/8 or [::1])")
	}
	if ap.Port() == 0 {
		return "", fmt.Errorf("--forward needs a port")
	}
	if what, bad := refusedPorts[ap.Port()]; bad {
		return "", fmt.Errorf("--forward may not be port %d (%s)", ap.Port(), what)
	}
	return netip.AddrPortFrom(ap.Addr().Unmap(), ap.Port()).String(), nil
}

// checkStateDir accepts only an absolute, clean path ending in remote/tailnet,
// so a slip in the caller can never point forget at another directory.
func checkStateDir(s string) (string, error) {
	if s == "" || !filepath.IsAbs(s) || filepath.Clean(s) != s {
		return "", errors.New("--state-dir must be an absolute, clean path")
	}
	if filepath.Base(s) != "tailnet" || filepath.Base(filepath.Dir(s)) != "remote" {
		return "", errors.New("--state-dir must end in remote/tailnet")
	}
	return s, nil
}

func checkStatusFile(s, stateDir string) (string, error) {
	if s == "" || !filepath.IsAbs(s) || filepath.Clean(s) != s {
		return "", errors.New("--status-file must be an absolute, clean path")
	}
	if strings.HasPrefix(s, stateDir+string(filepath.Separator)) || s == stateDir {
		return "", errors.New("--status-file may not be inside --state-dir")
	}
	return s, nil
}

var hostnameRE = regexp.MustCompile(`^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`)

func checkHostname(s string) (string, error) {
	if !hostnameRE.MatchString(s) {
		return "", errors.New("--hostname must be a DNS label: a to z, 0 to 9 and -, at most 63")
	}
	return s, nil
}

// checkControlURL accepts only a loopback http(s) URL: it exists so tests can
// point the node at a control plane that does not answer.
func checkControlURL(s string) (string, error) {
	if s == "" {
		return "", nil
	}
	u, err := url.Parse(s)
	if err != nil || (u.Scheme != "http" && u.Scheme != "https") {
		return "", errors.New("--test-control-url must be an http URL")
	}
	host := u.Hostname()
	ip, err := netip.ParseAddr(host)
	if err != nil || !ip.IsLoopback() {
		return "", errors.New("--test-control-url must name a loopback address")
	}
	return s, nil
}

func parseRun(args []string, stderr io.Writer) (*runConfig, error) {
	fs := flag.NewFlagSet("sbxw-tailnet", flag.ContinueOnError)
	fs.SetOutput(stderr)
	c := &runConfig{}
	fs.StringVar(&c.forward, "forward", "127.0.0.1:7683", "the daemon's remote listener, a loopback IP:PORT")
	fs.StringVar(&c.stateDir, "state-dir", "", "the node's state directory (absolute, ending in remote/tailnet)")
	fs.StringVar(&c.statusFile, "status-file", "", "where to write the status JSON")
	fs.StringVar(&c.hostname, "hostname", "sbxw", "the machine name in the tailnet (public in certificate logs)")
	fs.IntVar(&c.parentPID, "parent-pid", 0, "exit when this process is no longer the parent")
	fs.StringVar(&c.controlURL, "test-control-url", "", "tests only: a loopback control URL")
	fs.DurationVar(&c.loginWait, "login-wait", 60*time.Second, "how long to wait for a login link")
	fs.BoolVar(&c.push, "push", false, "read Web Push jobs on stdin and answer on stdout (API-push.md)")
	if err := fs.Parse(args); err != nil {
		return nil, err
	}
	if fs.NArg() != 0 {
		return nil, fmt.Errorf("unexpected argument %q", fs.Arg(0))
	}
	var err error
	if c.forward, err = checkForward(c.forward); err != nil {
		return nil, err
	}
	if c.stateDir, err = checkStateDir(c.stateDir); err != nil {
		return nil, err
	}
	if c.statusFile, err = checkStatusFile(c.statusFile, c.stateDir); err != nil {
		return nil, err
	}
	if c.hostname, err = checkHostname(c.hostname); err != nil {
		return nil, err
	}
	if c.controlURL, err = checkControlURL(c.controlURL); err != nil {
		return nil, err
	}
	if c.parentPID <= 1 {
		return nil, errors.New("--parent-pid is required")
	}
	if c.loginWait <= 0 {
		return nil, errors.New("--login-wait must be positive")
	}
	return c, nil
}

func parseForget(args []string, stderr io.Writer) (*forgetConfig, error) {
	fs := flag.NewFlagSet("sbxw-tailnet forget", flag.ContinueOnError)
	fs.SetOutput(stderr)
	c := &forgetConfig{}
	fs.StringVar(&c.stateDir, "state-dir", "", "the node's state directory")
	fs.StringVar(&c.statusFile, "status-file", "", "the status file to remove")
	fs.StringVar(&c.controlURL, "test-control-url", "", "tests only: a loopback control URL")
	if err := fs.Parse(args); err != nil {
		return nil, err
	}
	if fs.NArg() != 0 {
		return nil, fmt.Errorf("unexpected argument %q", fs.Arg(0))
	}
	var err error
	if c.stateDir, err = checkStateDir(c.stateDir); err != nil {
		return nil, err
	}
	if c.statusFile, err = checkStatusFile(c.statusFile, c.stateDir); err != nil {
		return nil, err
	}
	if c.controlURL, err = checkControlURL(c.controlURL); err != nil {
		return nil, err
	}
	return c, nil
}

// ensurePrivateDir creates dir with mode 0700 and refuses one that is a
// symlink or that another user owns.
func ensurePrivateDir(dir string) error {
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	fi, err := os.Lstat(dir)
	if err != nil {
		return err
	}
	if !fi.IsDir() {
		return fmt.Errorf("%s is not a directory", dir)
	}
	if err := checkOwner(fi); err != nil {
		return err
	}
	return os.Chmod(dir, 0o700)
}
