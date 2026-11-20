package main

import (
	"context"
	"crypto/tls"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"tailscale.com/client/local"
	"tailscale.com/client/tailscale/apitype"
	"tailscale.com/ipn"
	"tailscale.com/tailcfg"
	"tailscale.com/tsnet"
)

const adminDNS = "https://login.tailscale.com/admin/dns"

// node runs the tsnet server and the single HTTPS listener.
type node struct {
	cfg    *runConfig
	status *statusWriter
	logf   func(format string, args ...any) // stderr, state changes only

	mu   sync.Mutex
	name string // the tailnet name once running, lower case, no trailing dot
}

func (n *node) tailnetName() string {
	n.mu.Lock()
	defer n.mu.Unlock()
	return n.name
}

func (n *node) setName(s string) {
	n.mu.Lock()
	n.name = s
	n.mu.Unlock()
}

func (n *node) newServer() *tsnet.Server {
	return &tsnet.Server{
		Dir:        n.cfg.stateDir,
		Hostname:   n.cfg.hostname,
		ControlURL: n.cfg.controlURL,
		// Never an auth key: the first join is a login link (Q24).
		AuthKey:   "",
		Ephemeral: false,
		// tsnet prints the login URL to UserLogf every few seconds: drop it
		// all. The URL goes to the status file only.
		UserLogf: func(string, ...any) {},
		Logf:     nil, // backend logs are discarded
		// Never set: RunWebClient, AdvertiseTags, Port, Store, Tun.
	}
}

// run brings the node up and serves until ctx ends. It returns nil on a
// clean stop.
func (n *node) run(ctx context.Context) error {
	srv := n.newServer()
	defer srv.Close()
	if err := srv.Start(); err != nil {
		return fmt.Errorf("starting the Tailscale node: %w", err)
	}
	lc, err := srv.LocalClient()
	if err != nil {
		return fmt.Errorf("starting the Tailscale node: %w", err)
	}

	if err := n.waitRunning(ctx, lc); err != nil {
		return err
	}
	if ctx.Err() != nil {
		return nil
	}

	st, err := lc.StatusWithoutPeers(ctx)
	if err != nil {
		return fmt.Errorf("reading the node status: %w", err)
	}
	name := ""
	if st.Self != nil {
		name = strings.ToLower(strings.TrimSuffix(st.Self.DNSName, "."))
	}
	if !validTailnetName(name) {
		return errors.New("the tailnet gave this node no usable DNS name")
	}
	n.setName(name)

	// The owner of this node: only that user's own devices may connect (F7).
	var self tailcfg.UserID
	if st.Self != nil && (st.Self.Tags == nil || st.Self.Tags.Len() == 0) {
		self = st.Self.UserID
	}
	if self.IsZero() {
		return errors.New("this node has no owning user (tagged or unknown): refusing to serve")
	}

	// HTTPS is required: without it the phone's microphone does not work,
	// so there is no plain-HTTP fallback.
	if st.CurrentTailnet == nil || !st.CurrentTailnet.MagicDNSEnabled {
		n.status.set(stateHTTPSOff, name, "",
			"MagicDNS is off in this tailnet. Turn it on at "+adminDNS+", then turn remote access off and on.")
		<-ctx.Done()
		return nil
	}
	if !containsFold(st.CertDomains, name) {
		n.status.set(stateHTTPSOff, name, "",
			"HTTPS certificates are off in this tailnet. Turn on HTTPS at "+adminDNS+", then turn remote access off and on.")
		<-ctx.Done()
		return nil
	}

	// Fetch the certificate now, so a refusal shows here and not as a
	// failed handshake on the phone.
	certCtx, cancel := context.WithTimeout(ctx, 2*time.Minute)
	_, _, err = lc.CertPair(certCtx, name)
	cancel()
	if ctx.Err() != nil {
		return nil
	}
	if err != nil {
		n.status.set(stateHTTPSOff, name, "",
			"Tailscale did not issue an HTTPS certificate for "+name+". Check HTTPS at "+adminDNS+".")
		<-ctx.Done()
		return nil
	}

	// The one listener: TCP 443 on the tailnet only (Listen never uses
	// Funnel), wrapped in TLS with the tailnet certificate.
	raw, err := srv.Listen("tcp", ":443")
	if err != nil {
		return fmt.Errorf("listening on the tailnet: %w", err)
	}
	ln := tls.NewListener(raw, &tls.Config{
		MinVersion:     tls.VersionTLS12,
		NextProtos:     []string{"http/1.1"},
		GetCertificate: lc.GetCertificate,
	})
	px := newProxy(n.cfg.forward, n.tailnetName, whoIsVia(lc, self))
	px.onRefuse = func() { n.logf("refused a connection from a device that is not the node owner's") }
	hs := newHTTPServer(px)
	serveErr := make(chan error, 1)
	go func() { serveErr <- hs.Serve(ln) }()
	n.status.set(stateRunning, name, "", "")

	go n.watchLater(ctx, lc)

	select {
	case <-ctx.Done():
		sctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		_ = hs.Shutdown(sctx)
		cancel()
		_ = hs.Close()
		return nil
	case err := <-serveErr:
		if errors.Is(err, http.ErrServerClosed) {
			return nil
		}
		return fmt.Errorf("serving on the tailnet: %w", err)
	}
}

// waitRunning follows the IPN bus until the node is Running, reporting
// needs_login with the login link meanwhile.
func (n *node) waitRunning(ctx context.Context, lc *local.Client) error {
	w, err := lc.WatchIPNBus(ctx, ipn.NotifyInitialState)
	if err != nil {
		if ctx.Err() != nil {
			return nil
		}
		return fmt.Errorf("watching the node: %w", err)
	}
	defer w.Close()

	type event struct {
		n   ipn.Notify
		err error
	}
	events := make(chan event)
	go func() {
		for {
			nt, err := w.Next()
			select {
			case events <- event{nt, err}:
			case <-ctx.Done():
				return
			}
			if err != nil {
				return
			}
		}
	}()

	var state ipn.State = ipn.NoState
	loginURL := ""
	noLink := time.NewTimer(n.cfg.loginWait)
	defer noLink.Stop()
	poll := time.NewTicker(2 * time.Second)
	defer poll.Stop()

	report := func() {
		switch state {
		case ipn.NeedsLogin:
			if loginURL != "" {
				n.status.set(stateNeedsLogin, "", loginURL,
					"Open the link to add this Mac's sbxw node to your tailnet.")
			}
		case ipn.NeedsMachineAuth:
			n.status.set(stateNeedsLogin, "", "",
				"Approve the sbxw node on the Machines page of your Tailscale admin console.")
		}
	}

	for {
		select {
		case <-ctx.Done():
			return nil
		case ev := <-events:
			if ev.err != nil {
				if ctx.Err() != nil {
					return nil
				}
				return fmt.Errorf("watching the node: %w", ev.err)
			}
			if ev.n.ErrMessage != nil {
				return errors.New("the Tailscale node reported an error")
			}
			if ev.n.BrowseToURL != nil && safeLoginURL(*ev.n.BrowseToURL) {
				loginURL = *ev.n.BrowseToURL
			}
			if ev.n.State != nil {
				state = *ev.n.State
			}
			if state == ipn.Running {
				return nil
			}
			report()
		case <-poll.C:
			// The link may only show in the status, not on the bus.
			if state == ipn.NeedsLogin && loginURL == "" {
				if st, err := lc.StatusWithoutPeers(ctx); err == nil && safeLoginURL(st.AuthURL) {
					loginURL = st.AuthURL
					report()
				}
			}
		case <-noLink.C:
			if loginURL == "" && state != ipn.NeedsMachineAuth {
				n.status.set(stateError, "", "",
					"No login link from Tailscale yet: the coordination server may be unreachable. Still trying.")
			}
		}
	}
}

// watchLater reports a node that leaves Running (an expired key, a removal
// in the admin console) while the listener is up.
func (n *node) watchLater(ctx context.Context, lc *local.Client) {
	w, err := lc.WatchIPNBus(ctx, 0)
	if err != nil {
		return
	}
	defer w.Close()
	for {
		nt, err := w.Next()
		if err != nil {
			return
		}
		if nt.State == nil {
			continue
		}
		switch *nt.State {
		case ipn.Running:
			n.status.set(stateRunning, n.tailnetName(), "", "")
		case ipn.NeedsLogin, ipn.NeedsMachineAuth:
			n.status.set(stateError, "", "",
				"The sbxw node was logged out of the tailnet (its key expired or it was removed). Turn remote access off and on to log in again.")
		}
	}
}

// whoIsVia admits only the person's own devices (see admitPeer). The WhoIs
// answer is used for that decision only: never forwarded, never logged.
func whoIsVia(lc *local.Client, self tailcfg.UserID) whoIsFunc {
	return func(ctx context.Context, remoteAddr string) error {
		ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
		defer cancel()
		who, err := lc.WhoIs(ctx, remoteAddr)
		if err != nil {
			return errNotOwnDevice
		}
		return admitPeer(self, who)
	}
}

var errNotOwnDevice = errors.New("not one of the node owner's own devices")

// admitPeer accepts a peer only when it is a device of the same Tailscale
// user as this node: an untagged node whose owner (Node.User) and profile
// (UserProfile.ID) are this node's user, by stable numeric user id, and that
// was not shared into the tailnet (Node.Sharer zero). Tagged nodes, nodes of
// other users of the tailnet, shared-in nodes and anything WhoIs cannot fully
// identify are refused.
func admitPeer(self tailcfg.UserID, who *apitype.WhoIsResponse) error {
	if self.IsZero() || who == nil || who.Node == nil || who.UserProfile == nil {
		return errNotOwnDevice
	}
	nd := who.Node
	if nd.IsTagged() || nd.Sharer != 0 || nd.User.IsZero() {
		return errNotOwnDevice
	}
	if nd.User != self || who.UserProfile.ID != self {
		return errNotOwnDevice
	}
	return nil
}

// validTailnetName accepts a plain DNS name: letters, digits, '-' and '.',
// at most 253 characters, no empty label.
func validTailnetName(s string) bool {
	if s == "" || len(s) > 253 {
		return false
	}
	for _, label := range strings.Split(s, ".") {
		if label == "" || len(label) > 63 {
			return false
		}
		for _, c := range label {
			ok := c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '-'
			if !ok {
				return false
			}
		}
	}
	return true
}

// safeLoginURL keeps only an https URL with a host and no user info.
func safeLoginURL(s string) bool {
	if s == "" || len(s) > 2048 {
		return false
	}
	u, err := url.Parse(s)
	return err == nil && u.Scheme == "https" && u.Host != "" && u.User == nil
}

func containsFold(list []string, s string) bool {
	for _, v := range list {
		if strings.EqualFold(strings.TrimSuffix(v, "."), s) {
			return true
		}
	}
	return false
}
