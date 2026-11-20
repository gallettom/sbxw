package main

import (
	"context"
	"errors"
	"io"
	"strings"
	"testing"

	"tailscale.com/client/tailscale/apitype"
	"tailscale.com/tailcfg"
)

const owner tailcfg.UserID = 1001

func peer(user tailcfg.UserID, mod func(*tailcfg.Node)) *apitype.WhoIsResponse {
	n := &tailcfg.Node{Name: "phone.tail1234.ts.net.", User: user, StableID: "n1"}
	if mod != nil {
		mod(n)
	}
	return &apitype.WhoIsResponse{Node: n, UserProfile: &tailcfg.UserProfile{ID: user, LoginName: "someone@example.com"}}
}

func TestAdmitPeer(t *testing.T) {
	if err := admitPeer(owner, peer(owner, nil)); err != nil {
		t.Fatalf("own device refused: %v", err)
	}

	mismatch := peer(owner, nil)
	mismatch.UserProfile.ID = 2002

	refused := map[string]*apitype.WhoIsResponse{
		"another user of the tailnet":   peer(2002, nil),
		"a tagged node":                 peer(owner, func(n *tailcfg.Node) { n.Tags = []string{"tag:server"} }),
		"a tagged node of no user":      peer(0, func(n *tailcfg.Node) { n.Tags = []string{"tag:server"} }),
		"a node shared in by the owner": peer(owner, func(n *tailcfg.Node) { n.Sharer = owner }),
		"a node shared in by another":   peer(2002, func(n *tailcfg.Node) { n.Sharer = owner }),
		"a node with no user":           peer(0, nil),
		"profile and node disagree":     mismatch,
		"no node":                       {UserProfile: &tailcfg.UserProfile{ID: owner}},
		"no profile":                    {Node: &tailcfg.Node{User: owner}},
		"nothing":                       nil,
	}
	for what, who := range refused {
		if err := admitPeer(owner, who); err == nil {
			t.Errorf("%s: admitted", what)
		}
	}
	if err := admitPeer(0, peer(0, nil)); err == nil {
		t.Error("admitted with no owner known")
	}
}

// fakeWhoIsResp runs admitPeer on a canned WhoIs answer, as whoIsVia does on
// the real one.
func fakeWhoIsResp(who *apitype.WhoIsResponse, err error) whoIsFunc {
	return func(context.Context, string) error {
		if err != nil {
			return errNotOwnDevice
		}
		return admitPeer(owner, who)
	}
}

func TestProxyAdmitsOnlyOwnDevices(t *testing.T) {
	cases := []struct {
		what string
		who  *apitype.WhoIsResponse
		err  error
		want int
	}{
		{"own device", peer(owner, nil), nil, 200},
		{"another user", peer(2002, nil), nil, 403},
		{"tagged node", peer(owner, func(n *tailcfg.Node) { n.Tags = []string{"tag:x"} }), nil, 403},
		{"shared node", peer(2002, func(n *tailcfg.Node) { n.Sharer = 2002 }), nil, 403},
		{"WhoIs error", nil, errors.New("no such peer"), 403},
	}
	for _, c := range cases {
		var s seen
		up := newUpstream(t, &s)
		px := newProxy(strings.TrimPrefix(up.URL, "http://"), func() string { return testName }, fakeWhoIsResp(c.who, c.err))
		refusals := 0
		px.onRefuse = func() { refusals++ }
		front := newFrontWith(t, px)
		for i := 0; i < 3; i++ {
			res := do(t, front, get(t, "/r/pair/start"))
			if res.StatusCode != c.want {
				t.Errorf("%s: status %d, want %d", c.what, res.StatusCode, c.want)
			}
			b, _ := io.ReadAll(res.Body)
			if c.want == 403 && string(b) != "Forbidden.\n" {
				t.Errorf("%s: body %q", c.what, b)
			}
		}
		s.mu.Lock()
		reached := s.host != ""
		s.mu.Unlock()
		if reached != (c.want == 200) {
			t.Errorf("%s: reached upstream = %v", c.what, reached)
		}
		s.mu.Lock()
		for name := range s.header {
			if strings.HasPrefix(strings.ToLower(name), "x-sbxw-") {
				t.Errorf("%s: identity header %s forwarded", c.what, name)
			}
		}
		s.mu.Unlock()
		// Three refusals within a minute: one log line.
		if want := map[bool]int{true: 1, false: 0}[c.want == 403]; refusals != want {
			t.Errorf("%s: %d refusal notes, want %d", c.what, refusals, want)
		}
	}
}
