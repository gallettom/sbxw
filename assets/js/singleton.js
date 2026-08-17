// ── Single-tab gate ──────────────────────────────────────────────────────
//
// sbxw expects exactly one browser tab. A second one doesn't get its own
// slice of the two runtime worker threads, the PTY per sandbox, or the
// browser's six-per-origin HTTP/1.1 connection budget — it just contends
// with the first for all of it (see `api_stream` in src/web.rs). Tabs in the
// *same* browser can find each other instantly over `BroadcastChannel`
// (same-origin, cross-tab, no network round trip involved) — so before
// paying for any of that, a freshly opened tab asks "is anyone already
// here?" and, if so, offers to hand off instead of piling on.
//
// This can only catch same-browser duplicates: a second window in a
// different browser, profile, or device is invisible to BroadcastChannel.
// The header's `#multi-client-warning` badge (driven by the server's live
// tab count on `/api/stream`) is what catches that case, once both are
// already running.
//
// Everything the app actually needs — xterm.js and every local script — is
// loaded from here, and only once a decision is made. That's what makes a
// duplicate tab cheap: it never opens an SSE connection, a WebSocket, or
// pulls in xterm, unless the user explicitly chooses to.

const SBXW_APP_SCRIPTS = [
  'https://cdn.jsdelivr.net/npm/xterm@5.3.0/lib/xterm.min.js',
  'https://cdn.jsdelivr.net/npm/@xterm/addon-fit@0.10.0/lib/addon-fit.min.js',
  'https://cdn.jsdelivr.net/npm/@xterm/addon-canvas@0.7.0/lib/addon-canvas.min.js',
  '/js/util.js',
  '/js/panes.js',
  '/js/pane-controls.js',
  '/js/sandboxes.js',
  '/js/create.js',
  '/js/ports.js',
  '/js/files.js',
  '/js/ssh.js',
  '/js/lifecycle.js',
  '/js/main.js',
  // After main.js: the relay popup subscribes to the SSE stream that file
  // opens, and paints its buttons from the sandbox list sandboxes.js holds.
  '/js/relay.js',
];

function sbxwLoadScript(src) {
  return new Promise((resolve, reject) => {
    const s = document.createElement('script');
    s.src = src;
    s.onload = resolve;
    s.onerror = () => reject(new Error(`sbxw: failed to load ${src}`));
    document.body.appendChild(s);
  });
}

// Scripts share one global scope and depend on load order (main.js reads
// globals `panes.js` and friends declare), so this loads them one at a time
// rather than in parallel — the same order they used to run in as static tags.
function sbxwLoadApp() {
  SBXW_APP_SCRIPTS
    .reduce((chain, src) => chain.then(() => sbxwLoadScript(src)), Promise.resolve())
    .catch(err => console.error(err));
}

// Older browsers without BroadcastChannel have no way to detect a duplicate
// tab — just boot normally, same as sbxw did before this existed.
if (typeof BroadcastChannel === 'undefined') {
  sbxwLoadApp();
} else {
  const bc = new BroadcastChannel('sbxw-singleton');
  const myId = Math.random().toString(36).slice(2);
  let heardPong = false;
  // Only a tab that has actually decided to run the app answers pings or
  // focus-requests — a tab still mid-decision (showing its own interstitial)
  // has no app running here to hand off to. Two tabs opened within the same
  // ~200ms window can both miss each other and both boot; that race is
  // accepted rather than solved, same tradeoff any peer-election over a
  // fixed timeout makes.
  let isRunning = false;

  const flashPage = () => {
    document.documentElement.classList.add('sbxw-flash');
    setTimeout(() => document.documentElement.classList.remove('sbxw-flash'), 900);
  };

  bc.onmessage = ev => {
    const msg = ev.data;
    if (!msg) return;
    if (msg.type === 'ping' && isRunning) {
      bc.postMessage({ type: 'pong', id: myId });
    } else if (msg.type === 'pong' && msg.id !== myId) {
      heardPong = true;
    } else if (msg.type === 'focus-request' && isRunning) {
      window.focus();
      flashPage();
    }
  };

  // Clicking the header's "N tabs open" warning asks any other running tab
  // to raise its hand — the same handoff the interstitial below offers, but
  // reachable after a tab has already chosen to boot despite a duplicate.
  document.getElementById('multi-client-warning')
    ?.addEventListener('click', () => bc.postMessage({ type: 'focus-request' }));

  bc.postMessage({ type: 'ping', id: myId });

  setTimeout(() => {
    if (!heardPong) {
      isRunning = true;
      sbxwLoadApp();
      return;
    }

    // Another tab is already running — offer to hand off instead of booting
    // a second full app here.
    const overlay = document.getElementById('singleton-overlay');
    overlay.hidden = false;

    document.getElementById('singleton-focus').addEventListener('click', ev => {
      bc.postMessage({ type: 'focus-request' });
      const btn = ev.currentTarget;
      btn.textContent = 'Sent — you can close this tab';
      btn.disabled = true;
    });

    document.getElementById('singleton-anyway').addEventListener('click', () => {
      overlay.hidden = true;
      isRunning = true;
      sbxwLoadApp();
    });
  }, 200);
}
