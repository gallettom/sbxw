package main

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// The states of API.md section 6.
const (
	stateStarting   = "starting"
	stateNeedsLogin = "needs_login"
	stateRunning    = "running"
	stateHTTPSOff   = "https_off"
	stateStopped    = "stopped"
	stateError      = "error"
)

const (
	maxStatusBytes = 8 << 10
	heartbeat      = 5 * time.Second
)

// status is the whole content of the status file: these six fields and no
// other. It never holds a key of any kind.
type status struct {
	State     string  `json:"state"`
	Name      *string `json:"name"`
	LoginURL  *string `json:"login_url"`
	Detail    *string `json:"detail"`
	PID       int     `json:"pid"`
	UpdatedMS int64   `json:"updated_ms"`
}

func strp(s string) *string {
	if s == "" {
		return nil
	}
	return &s
}

// statusWriter owns the status file. Every write goes to a temporary file in
// the same directory, mode 0600, then is renamed over the old one.
type statusWriter struct {
	path string
	now  func() time.Time
	pid  int

	mu  sync.Mutex
	cur status
	// onChange is called with the new state name after a state change (for
	// the stderr line). Never with the URL.
	onChange func(state string)
}

func newStatusWriter(path string) *statusWriter {
	return &statusWriter{path: path, now: time.Now, pid: os.Getpid()}
}

// set records a new status and writes it. loginURL is kept only in
// needs_login; name is cleared outside running and https_off.
func (w *statusWriter) set(state, name, loginURL, detail string) error {
	w.mu.Lock()
	defer w.mu.Unlock()
	if state != stateNeedsLogin {
		loginURL = ""
	}
	if state != stateRunning && state != stateHTTPSOff {
		name = ""
	}
	changed := w.cur.State != state
	w.cur = status{State: state, Name: strp(name), LoginURL: strp(loginURL), Detail: strp(detail)}
	err := w.writeLocked()
	if changed && w.onChange != nil {
		w.onChange(state)
	}
	return err
}

// beat rewrites the current status with a fresh updated_ms.
func (w *statusWriter) beat() error {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.cur.State == "" {
		return nil
	}
	return w.writeLocked()
}

func (w *statusWriter) state() string {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.cur.State
}

func (w *statusWriter) writeLocked() error {
	w.cur.PID = w.pid
	w.cur.UpdatedMS = w.now().UnixMilli()
	b, err := json.Marshal(w.cur)
	if err != nil {
		return err
	}
	if len(b) > maxStatusBytes {
		// Only a very long detail or URL can do this: drop the detail.
		w.cur.Detail = strp("detail too long")
		if b, err = json.Marshal(w.cur); err != nil {
			return err
		}
		if len(b) > maxStatusBytes {
			return errors.New("status too large")
		}
	}
	dir := filepath.Dir(w.path)
	f, err := os.CreateTemp(dir, ".tailnet-status-*")
	if err != nil {
		return err
	}
	tmp := f.Name()
	defer os.Remove(tmp) // a no-op after the rename
	if err := f.Chmod(0o600); err != nil {
		f.Close()
		return err
	}
	if _, err := f.Write(append(b, '\n')); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	return os.Rename(tmp, w.path)
}

// remove deletes the status file (after a clean stop).
func (w *statusWriter) remove() error {
	w.mu.Lock()
	defer w.mu.Unlock()
	err := os.Remove(w.path)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	return err
}
