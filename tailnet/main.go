// Command sbxw-tailnet joins the person's tailnet as its own node and
// forwards HTTPS on that node's port 443 to the sbxw daemon's remote listener
// on loopback. It is the only door from outside: it serves nothing else.
// See README.md.
package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"tailscale.com/envknob"
	"tailscale.com/ipn"
	"tailscale.com/tsnet"
)

func main() {
	os.Exit(realMain(os.Args[1:], os.Stderr))
}

func realMain(args []string, stderr io.Writer) int {
	logf := func(format string, a ...any) {
		fmt.Fprintf(stderr, "sbxw-tailnet: "+format+"\n", a...)
	}
	if err := checkEnv(os.Environ()); err != nil {
		logf("%v", err)
		return 2
	}
	// Belt and braces with the ts_omit_logtail build tag: no log upload to
	// log.tailscale.com.
	envknob.SetNoLogsNoSupport()

	if len(args) > 0 && args[0] == "forget" {
		cfg, err := parseForget(args[1:], stderr)
		if err != nil {
			logf("%v", err)
			return 2
		}
		if err := forget(cfg, logf); err != nil {
			logf("forget: %v", err)
			return 1
		}
		logf("forgotten")
		return 0
	}

	cfg, err := parseRun(args, stderr)
	if err != nil {
		logf("%v", err)
		return 2
	}
	return run(cfg, logf)
}

// checkEnv refuses to start with any TS_* or TSNET_* variable set: tsnet
// reads auth keys, OAuth secrets, a control URL and debug switches from them,
// some at package initialisation, before main could clear them. The daemon
// spawns this program with a cleared environment.
func checkEnv(env []string) error {
	var bad []string
	for _, kv := range env {
		name, _, _ := strings.Cut(kv, "=")
		upper := strings.ToUpper(name)
		if strings.HasPrefix(upper, "TS_") || strings.HasPrefix(upper, "TSNET_") {
			bad = append(bad, name)
		}
	}
	if len(bad) > 0 {
		return fmt.Errorf("refusing to start with Tailscale environment variables set: %s", strings.Join(bad, ", "))
	}
	return nil
}

func run(cfg *runConfig, logf func(string, ...any)) int {
	if err := ensurePrivateDir(filepath.Dir(cfg.statusFile)); err != nil {
		logf("status directory: %v", err)
		return 1
	}
	if err := ensurePrivateDir(cfg.stateDir); err != nil {
		logf("state directory: %v", err)
		return 1
	}
	sw := newStatusWriter(cfg.statusFile)
	sw.onChange = func(state string) { logf("state %s", state) }

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer stop()
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	go watchParent(ctx, cancel, cfg.parentPID)
	go func() {
		t := time.NewTicker(heartbeat)
		defer t.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-t.C:
				_ = sw.beat()
			}
		}
	}()

	if err := sw.set(stateStarting, "", "", ""); err != nil {
		logf("status file: %v", err)
		return 1
	}
	n := &node{cfg: cfg, status: sw, logf: logf}
	err := n.run(ctx)
	cancel()
	if err != nil {
		_ = sw.set(stateError, "", "", sentence(err))
		logf("stopped on an error")
		return 1
	}
	_ = sw.set(stateStopped, "", "", "")
	_ = sw.remove()
	return 0
}

// sentence turns an error into the status file's plain detail.
func sentence(err error) string {
	s := err.Error()
	if len(s) > 500 {
		s = s[:500]
	}
	return s
}

// watchParent cancels when the process that started us is gone: on macOS and
// Linux an orphan is re-parented, so getppid changes.
func watchParent(ctx context.Context, cancel context.CancelFunc, parent int) {
	t := time.NewTicker(time.Second)
	defer t.Stop()
	for {
		if os.Getppid() != parent {
			cancel()
			return
		}
		select {
		case <-ctx.Done():
			return
		case <-t.C:
		}
	}
}

// forget logs the node out (expiring its key at Tailscale) when it can, then
// deletes its state directory and the status file.
func forget(cfg *forgetConfig, logf func(string, ...any)) error {
	fi, err := os.Lstat(cfg.stateDir)
	if errors.Is(err, os.ErrNotExist) {
		_ = removeIfExists(cfg.statusFile)
		return nil
	}
	if err != nil {
		return err
	}
	if !fi.IsDir() {
		return fmt.Errorf("%s is not a directory", cfg.stateDir)
	}
	if err := checkOwner(fi); err != nil {
		return err
	}
	if _, err := os.Stat(filepath.Join(cfg.stateDir, "tailscaled.state")); err == nil {
		if err := logout(cfg); err != nil {
			logf("could not log the node out (%v); remove it on the Machines page of the Tailscale admin console", err)
		} else {
			logf("logged out")
		}
	}
	if err := os.RemoveAll(cfg.stateDir); err != nil {
		return err
	}
	return removeIfExists(cfg.statusFile)
}

func logout(cfg *forgetConfig) error {
	srv := &tsnet.Server{
		Dir:        cfg.stateDir,
		ControlURL: cfg.controlURL,
		UserLogf:   func(string, ...any) {},
	}
	defer srv.Close()
	if err := srv.Start(); err != nil {
		return err
	}
	lc, err := srv.LocalClient()
	if err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	// Logout needs the node to talk to control: wait until it is Running.
	w, err := lc.WatchIPNBus(ctx, ipn.NotifyInitialState)
	if err != nil {
		return err
	}
	for {
		nt, err := w.Next()
		if err != nil {
			w.Close()
			return errors.New("the node did not come up in time")
		}
		if nt.State != nil && *nt.State == ipn.Running {
			break
		}
		if nt.State != nil && (*nt.State == ipn.NeedsLogin || *nt.State == ipn.NeedsMachineAuth) {
			w.Close()
			return errors.New("the node is already logged out")
		}
	}
	w.Close()
	return lc.Logout(ctx)
}

func removeIfExists(path string) error {
	err := os.Remove(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	return err
}
