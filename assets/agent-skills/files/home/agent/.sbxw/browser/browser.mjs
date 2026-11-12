#!/usr/bin/env node
// The sandbox's own browser: one headless Chromium the agent drives and the
// person watches from sbxw's Browser panel.
//
// Chromium listens for commands on 127.0.0.1:9222 only. That port has no
// password (whoever reaches it owns the browser), so it is never published:
// the person's view goes through `serve`, which speaks a narrow line protocol
// on stdin/stdout (pictures out, clicks and keys in) to the sbxw daemon.
//
// Everything Chromium loads goes through the sandbox's proxy, so the network
// policy applies to it as to any other program here.
//
//   node browser.mjs start            start Chromium if it is not running
//   node browser.mjs open <url>       load a page in the tab the person sees
//   node browser.mjs url              the current address and title
//   node browser.mjs text             the page's visible text
//   node browser.mjs eval <js>        evaluate an expression in the page
//   node browser.mjs click <selector> click the middle of an element
//   node browser.mjs type <selector> <text>
//   node browser.mjs shot <file.png>  a screenshot
//   node browser.mjs viewport [mobile|tablet|desktop|<w>x<h>|auto]
//                                     lay pages out as that device (auto: the panel's size)
//   node browser.mjs stop             close Chromium
//   node browser.mjs serve            the daemon's end (not for the agent)
//
// No dependencies: Node 22's fetch and WebSocket.

import { spawn } from 'node:child_process';
import { createHash, X509Certificate } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const PORT = 9222;
const BASE = `http://127.0.0.1:${PORT}`;
const HOME = os.homedir();
const PROFILE = path.join(HOME, '.sbxw', 'browser', 'profile');
const LAST_URL = path.join(HOME, '.sbxw', 'browser', 'last-url');
const FRAME_GAP_MS = 80; // at most ~12 pictures a second
const SIZE = { w: 1280, h: 800 };

// ── Viewport ────────────────────────────────────────────────────────────────
// A device to lay pages out as, kept in a file so it holds across the
// agent's commands (each its own DevTools session, whose overrides end with
// it) and the panel's. No file: the panel's size decides, as before.
const VIEWPORT = path.join(HOME, '.sbxw', 'browser', 'viewport');
const PRESETS = {
  mobile: { name: 'mobile', w: 390, h: 844, mobile: true, dpr: 3 },
  tablet: { name: 'tablet', w: 820, h: 1180, mobile: true, dpr: 2 },
  desktop: { name: 'desktop', w: 1280, h: 800, mobile: false, dpr: 1 },
};

function parseViewport(raw) {
  const v = String(raw || '').trim().toLowerCase();
  if (PRESETS[v]) return PRESETS[v];
  const m = v.match(/^(\d{3,4})\s*x\s*(\d{3,4})(\s+mobile)?$/);
  if (!m) return null;
  const w = Math.min(2560, Math.max(240, +m[1]));
  const h = Math.min(2560, Math.max(240, +m[2]));
  return { name: `${w}x${h}${m[3] ? ' mobile' : ''}`, w, h, mobile: !!m[3], dpr: m[3] ? 2 : 1 };
}

function readViewport() {
  try {
    const v = JSON.parse(fs.readFileSync(VIEWPORT, 'utf8'));
    return v && v.w && v.h ? v : null;
  } catch (_) {
    return null;
  }
}

function writeViewport(vp) {
  try {
    if (!vp) fs.rmSync(VIEWPORT, { force: true });
    else { fs.mkdirSync(path.dirname(VIEWPORT), { recursive: true }); fs.writeFileSync(VIEWPORT, JSON.stringify(vp)); }
  } catch (_) { /* best effort */ }
}

// What a phone's browser says it is, from this Chromium's own version, so
// sites that look at the user agent serve their mobile page too.
function mobileAgent(browser) {
  const ver = (String(browser || '').match(/\/([\d.]+)/) || [])[1] || '131.0.0.0';
  return `Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/${ver} Mobile Safari/537.36`;
}

async function applyViewport(cdp, sid, vp, v) {
  const metrics = vp
    ? { width: vp.w, height: vp.h, deviceScaleFactor: vp.dpr || 1, mobile: !!vp.mobile }
    : null;
  // A device outlives the session that set it, and clearing the override
  // does not bring the old width back in headless Chromium: auto sets the
  // default desktop size outright (the panel then sets its own).
  await cdp.send('Emulation.setDeviceMetricsOverride',
    metrics || { width: SIZE.w, height: SIZE.h, deviceScaleFactor: 1, mobile: false }, sid).catch(() => {});
  await cdp.send('Emulation.setTouchEmulationEnabled', vp && vp.mobile ? { enabled: true, maxTouchPoints: 5 } : { enabled: false }, sid).catch(() => {});
  const ua = vp && vp.mobile ? mobileAgent(v && v.Browser) : (v && v['User-Agent']) || '';
  if (ua) await cdp.send('Emulation.setUserAgentOverride', { userAgent: ua }, sid).catch(() => {});
}

// ── Chromium ────────────────────────────────────────────────────────────────

function findChromium() {
  const root = process.env.PLAYWRIGHT_BROWSERS_PATH || path.join(HOME, '.cache', 'ms-playwright');
  let dirs = [];
  try { dirs = fs.readdirSync(root).sort().reverse(); } catch (_) { /* none */ }
  for (const [prefix, bin, extra] of [
    ['chromium_headless_shell-', 'headless_shell', []],
    ['chromium-', 'chrome', ['--headless=new']],
  ]) {
    for (const d of dirs.filter((d) => d.startsWith(prefix))) {
      for (const sub of ['chrome-linux', 'chrome-linux64', 'chrome-headless-shell-linux64', 'chrome-headless-shell-linux-arm64']) {
        for (const name of [bin, 'chrome-headless-shell']) {
          const p = path.join(root, d, sub, name);
          if (fs.existsSync(p)) return { bin: p, extra };
        }
      }
    }
  }
  return null;
}

// The sandbox proxy re-signs HTTPS with its own authority, which Chromium
// does not take from the system store. Trust that one key, nothing wider.
function proxyTrust() {
  const b64 = process.env.PROXY_CA_CERT_B64;
  if (!b64) return [];
  try {
    const cert = new X509Certificate(Buffer.from(b64, 'base64'));
    const spki = cert.publicKey.export({ type: 'spki', format: 'der' });
    return [`--ignore-certificate-errors-spki-list=${createHash('sha256').update(spki).digest('base64')}`];
  } catch (_) {
    return [];
  }
}

// The page the browser was on when it was last closed: where it reopens.
function lastUrl() {
  try { return cleanUrl(fs.readFileSync(LAST_URL, 'utf8')) || 'about:blank'; } catch (_) { return 'about:blank'; }
}
function rememberUrl(url) {
  if (!/^https?:/.test(url || '')) return;
  try { fs.mkdirSync(path.dirname(LAST_URL), { recursive: true }); fs.writeFileSync(LAST_URL, url); } catch (_) { /* best effort */ }
}

async function version() {
  try {
    const r = await fetch(`${BASE}/json/version`, { signal: AbortSignal.timeout(1500) });
    return r.ok ? await r.json() : null;
  } catch (_) {
    return null;
  }
}

async function ensure() {
  let v = await version();
  if (v) return v;
  const found = findChromium();
  if (!found) {
    throw new Error('no Chromium in this sandbox: add "sbxw:agent-skills" to `kits` in sbxw.toml, then recreate the sandbox');
  }
  fs.mkdirSync(PROFILE, { recursive: true });
  const args = [
    ...found.extra,
    `--remote-debugging-port=${PORT}`,
    '--remote-debugging-address=127.0.0.1',
    `--user-data-dir=${PROFILE}`,
    `--window-size=${SIZE.w},${SIZE.h}`,
    '--no-sandbox',
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-dev-shm-usage',
    '--hide-scrollbars',
    ...proxyTrust(),
    lastUrl(),
  ];
  const child = spawn(found.bin, args, { detached: true, stdio: 'ignore' });
  child.unref();
  for (let i = 0; i < 60; i++) {
    await new Promise((r) => setTimeout(r, 250));
    v = await version();
    if (v) return v;
  }
  throw new Error('Chromium did not start within 15 seconds');
}

// ── DevTools protocol over one socket ───────────────────────────────────────

async function connect(wsUrl) {
  const ws = new WebSocket(wsUrl);
  await new Promise((ok, no) => {
    ws.addEventListener('open', ok, { once: true });
    ws.addEventListener('error', () => no(new Error('cannot reach Chromium')), { once: true });
  });
  let next = 1;
  const waiting = new Map();
  const listeners = [];
  ws.addEventListener('message', (m) => {
    let msg;
    try { msg = JSON.parse(m.data); } catch (_) { return; }
    if (msg.id && waiting.has(msg.id)) {
      const { ok, no } = waiting.get(msg.id);
      waiting.delete(msg.id);
      msg.error ? no(new Error(msg.error.message)) : ok(msg.result);
    } else if (msg.method) {
      for (const l of listeners) l(msg.method, msg.params || {}, msg.sessionId);
    }
  });
  return {
    ws,
    on: (l) => listeners.push(l),
    // An answer that never comes (a tab closed under the call) must not
    // hold up what waits on it for good.
    send: (method, params = {}, sessionId) =>
      new Promise((ok, no) => {
        const id = next++;
        const late = setTimeout(() => { waiting.delete(id); no(new Error(`${method}: no answer from Chromium`)); }, 8000);
        waiting.set(id, {
          ok: (v) => { clearTimeout(late); ok(v); },
          no: (e) => { clearTimeout(late); no(e); },
        });
        ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
      }),
  };
}

function cleanUrl(raw) {
  let u = String(raw || '').trim();
  if (!u) return null;
  if (u === 'about:blank') return u;
  if (!/^[a-z][a-z0-9+.-]*:/i.test(u)) u = `https://${u}`;
  try {
    const p = new URL(u);
    return p.protocol === 'http:' || p.protocol === 'https:' ? p.href : null;
  } catch (_) {
    return null;
  }
}

// What the sandbox proxy answers when the policy refuses a request.
function blockKind(body) {
  const first = String(body || '').trim().split('\n')[0];
  if (/^Approval required for /.test(first)) return 'approval';
  if (/^Blocked by local rule/.test(first)) return 'rule';
  if (/^Blocked by org policy/.test(first)) return 'org';
  if (/^Blocked by network policy/.test(first)) return 'policy';
  return null;
}

// ── serve: the daemon's end ─────────────────────────────────────────────────

async function serve() {
  const out = (o) => process.stdout.write(JSON.stringify(o) + '\n');
  let v;
  try {
    v = await ensure();
  } catch (e) {
    out({ t: 'error', msg: e.message });
    process.exit(1);
  }
  const cdp = await connect(v.webSocketDebuggerUrl);
  const pages = new Map(); // sessionId → { targetId, url, title }
  const requests = new Map(); // requestId → url, for failures
  let shown = null;
  let size = { ...SIZE };
  let pending = null;
  let lastSent = 0;
  let clogged = false;
  process.stdout.on('drain', () => { clogged = false; });

  const say = () => {
    const p = pages.get(shown);
    if (!p) return;
    out({ t: 'url', url: p.url || '', title: p.title || '' });
    rememberUrl(p.url);
  };
  let viewport = readViewport();
  const castSize = () => (viewport ? { w: viewport.w, h: viewport.h } : size);
  const cast = (sid) =>
    cdp.send('Page.startScreencast', { format: 'jpeg', quality: 60, maxWidth: castSize().w, maxHeight: castSize().h, everyNthFrame: 1 }, sid).catch(() => {});
  const fit = async (sid) => {
    if (viewport) return applyViewport(cdp, sid, viewport, v);
    await applyViewport(cdp, sid, null, v);
    await cdp.send('Emulation.setDeviceMetricsOverride', { width: size.w, height: size.h, deviceScaleFactor: 1, mobile: false }, sid).catch(() => {});
  };
  const sayViewport = () => out({ t: 'viewport', name: viewport ? viewport.name : 'auto' });
  const refit = async () => {
    if (!shown) return;
    await fit(shown);
    await cdp.send('Page.stopScreencast', {}, shown).catch(() => {});
    await cast(shown);
  };
  // The agent changes the device from its own commands: follow the file.
  // Its sessions' overrides also end with them, so a set device is put back
  // now and then (the same values again change nothing on the page).
  let viewportText = JSON.stringify(viewport);
  setInterval(async () => {
    const now = readViewport();
    const text = JSON.stringify(now);
    if (text !== viewportText) {
      viewportText = text;
      viewport = now;
      sayViewport();
      await refit();
    } else if (viewport && shown) {
      await fit(shown);
    }
  }, 1000).unref();
  const show = async (sid) => {
    if (shown === sid || !pages.has(sid)) return;
    if (shown) cdp.send('Page.stopScreencast', {}, shown).catch(() => {});
    shown = sid;
    // The address first, and the picture whatever the sizing does: a call
    // that gets no answer must not leave the panel blank.
    say();
    await Promise.race([fit(sid), new Promise((r) => setTimeout(r, 2000))]);
    await cast(sid);
    wake(sid);
  };
  // A tab nobody was watching can be asleep: it sends no picture and
  // answers nothing until it is brought to the front. If no picture came
  // soon after the tab was shown, wake it and ask for the pictures again.
  let framesOf = null;
  const wake = (sid) => {
    framesOf = null;
    setTimeout(async () => {
      if (shown !== sid || framesOf === sid) return;
      const page = pages.get(sid);
      if (page) cdp.send('Target.activateTarget', { targetId: page.targetId }).catch(() => {});
      cdp.send('Page.bringToFront', {}, sid).catch(() => {});
      cdp.send('Page.stopScreencast', {}, sid).catch(() => {});
      await cast(sid);
    }, 1500);
  };
  const flush = () => {
    if (!pending || clogged) return;
    const now = Date.now();
    if (now - lastSent < FRAME_GAP_MS) return;
    lastSent = now;
    clogged = !process.stdout.write(JSON.stringify(pending) + '\n');
    pending = null;
  };
  setInterval(flush, FRAME_GAP_MS).unref();

  // One page asks a refused host many times over: say it once in a while.
  const told = new Map();
  const blocked = (url, kind) => {
    let host = '';
    try { host = new URL(url).hostname; } catch (_) { return; }
    const now = Date.now();
    if (!host || now - (told.get(host) || 0) < 3000) return;
    told.set(host, now);
    if (told.size > 500) told.delete(told.keys().next().value);
    out({ t: 'blocked', host, kind });
  };

  cdp.on(async (method, p, sid) => {
    switch (method) {
      case 'Target.targetCreated':
        if (p.targetInfo.type === 'page') {
          cdp.send('Target.attachToTarget', { targetId: p.targetInfo.targetId, flatten: true }).catch(() => {});
        }
        break;
      case 'Target.attachedToTarget': {
        if (p.targetInfo.type !== 'page') break;
        pages.set(p.sessionId, { targetId: p.targetInfo.targetId, url: p.targetInfo.url, title: p.targetInfo.title });
        // Not waited for: a busy page answers these late (seconds, at
        // times), and the picture and the address do not depend on them.
        cdp.send('Page.enable', {}, p.sessionId).catch(() => {});
        cdp.send('Network.enable', {}, p.sessionId).catch(() => {});
        // The newest tab is the one being worked in.
        await show(p.sessionId);
        break;
      }
      case 'Target.detachedFromTarget': {
        pages.delete(p.sessionId);
        if (shown === p.sessionId) {
          shown = null;
          const rest = [...pages.keys()];
          if (rest.length) await show(rest[rest.length - 1]);
          else out({ t: 'url', url: '', title: '' });
        }
        break;
      }
      case 'Target.targetInfoChanged': {
        for (const [s, page] of pages) {
          if (page.targetId === p.targetInfo.targetId) {
            page.url = p.targetInfo.url;
            page.title = p.targetInfo.title;
            if (s === shown) say();
          }
        }
        break;
      }
      case 'Page.frameNavigated':
        if (!p.frame.parentId && pages.has(sid)) {
          pages.get(sid).url = p.frame.url;
          // A tab that goes somewhere is the one to watch.
          if (sid !== shown) await show(sid);
          else say();
        }
        break;
      case 'Page.navigatedWithinDocument':
        if (pages.has(sid)) {
          pages.get(sid).url = p.url;
          if (sid === shown) say();
        }
        break;
      case 'Page.screencastFrame':
        cdp.send('Page.screencastFrameAck', { sessionId: p.sessionId }, sid).catch(() => {});
        if (sid === shown) {
          framesOf = sid;
          pending = { t: 'frame', d: p.data, w: p.metadata.deviceWidth, h: p.metadata.deviceHeight };
          flush();
        }
        break;
      case 'Network.requestWillBeSent':
        requests.set(p.requestId, p.request.url);
        if (requests.size > 2000) requests.delete(requests.keys().next().value);
        break;
      case 'Network.responseReceived': {
        const r = p.response;
        if (r.status !== 403 || !/^text\/plain/i.test(r.mimeType || '')) break;
        // Only the proxy's own refusal counts: a site may answer 403 too.
        let kind = null;
        try {
          const b = await cdp.send('Network.getResponseBody', { requestId: p.requestId }, sid);
          kind = blockKind(b.base64Encoded ? Buffer.from(b.body, 'base64').toString() : b.body);
          if (!kind) break;
        } catch (_) {
          // No body to read (a blocked script or image): the proxy's refusal
          // carries no Server header, a site's nearly always does.
          const names = Object.keys(r.headers || {}).map((h) => h.toLowerCase());
          if (names.includes('server') || names.includes('via')) break;
          kind = 'approval';
        }
        blocked(r.url, kind);
        break;
      }
      case 'Network.loadingFailed':
        if (/TUNNEL_CONNECTION_FAILED/.test(p.errorText || '') && requests.has(p.requestId)) {
          blocked(requests.get(p.requestId), 'approval');
        }
        requests.delete(p.requestId);
        break;
      case 'Network.loadingFinished':
        requests.delete(p.requestId);
        break;
    }
  });

  cdp.ws.addEventListener('close', () => {
    out({ t: 'closed' });
    process.exit(0);
  });
  await cdp.send('Target.setDiscoverTargets', { discover: true });
  const { targetInfos } = await cdp.send('Target.getTargets');
  if (!targetInfos.some((t) => t.type === 'page')) {
    await cdp.send('Target.createTarget', { url: 'about:blank' });
  }
  out({ t: 'ready', browser: v.Browser || '' });
  sayViewport();
  // The tab is attached as Chromium announces it. If none is shown soon
  // after (an announcement missed, an attach that got no answer), ask for
  // the tabs and attach one outright: the panel must not stay blank.
  setTimeout(async () => {
    if (shown) return;
    try {
      const { targetInfos: now } = await cdp.send('Target.getTargets');
      const page = now.find((t) => t.type === 'page') || { targetId: (await cdp.send('Target.createTarget', { url: lastUrl() })).targetId };
      await cdp.send('Target.attachToTarget', { targetId: page.targetId, flatten: true });
    } catch (e) {
      out({ t: 'note', msg: `the browser's tab could not be shown: ${e.message}` });
    }
  }, 2000);

  const MOUSE = new Set(['mousePressed', 'mouseReleased', 'mouseMoved', 'mouseWheel']);
  const KEY = new Set(['keyDown', 'keyUp', 'rawKeyDown', 'char']);
  const num = (x, max) => Math.max(0, Math.min(max, Number(x) || 0));
  const history = async (step) => {
    const h = await cdp.send('Page.getNavigationHistory', {}, shown);
    const e = h.entries[h.currentIndex + step];
    if (e) await cdp.send('Page.navigateToHistoryEntry', { entryId: e.id }, shown);
  };
  const act = async (m) => {
    if (m.t === 'quit') {
      await cdp.send('Browser.close').catch(() => {});
      return;
    }
    // The first tab attaches a moment after the socket opens.
    for (let i = 0; i < 40 && !shown; i++) await new Promise((r) => setTimeout(r, 50));
    if (!shown) return;
    switch (m.t) {
      case 'mouse':
        if (!MOUSE.has(m.type)) return;
        await cdp.send('Input.dispatchMouseEvent', {
          type: m.type,
          x: num(m.x, 20000),
          y: num(m.y, 20000),
          button: ['left', 'middle', 'right'].includes(m.button) ? m.button : 'none',
          clickCount: num(m.clickCount, 3),
          deltaX: Number(m.deltaX) || 0,
          deltaY: Number(m.deltaY) || 0,
          modifiers: num(m.modifiers, 15),
        }, shown);
        break;
      case 'key':
        if (!KEY.has(m.type)) return;
        await cdp.send('Input.dispatchKeyEvent', {
          type: m.type,
          key: String(m.key || '').slice(0, 32),
          code: String(m.code || '').slice(0, 32),
          text: m.text ? String(m.text).slice(0, 8) : undefined,
          windowsVirtualKeyCode: num(m.vk, 255),
          modifiers: num(m.modifiers, 15),
        }, shown);
        break;
      case 'paste':
        await cdp.send('Input.insertText', { text: String(m.text || '').slice(0, 100000) }, shown);
        break;
      case 'nav': {
        const url = cleanUrl(m.url);
        if (url) await cdp.send('Page.navigate', { url }, shown);
        else out({ t: 'note', msg: 'not an http(s) address' });
        break;
      }
      case 'back': await history(-1); break;
      case 'forward': await history(1); break;
      case 'reload': await cdp.send('Page.reload', {}, shown); break;
      case 'size':
        size = { w: Math.round(num(m.w, 2560)) || SIZE.w, h: Math.round(num(m.h, 1600)) || SIZE.h };
        if (!viewport) await refit();
        break;
      case 'viewport': {
        const want = m.value === 'auto' ? null : parseViewport(m.value);
        if (m.value !== 'auto' && !want) return;
        viewport = want;
        viewportText = JSON.stringify(viewport);
        writeViewport(viewport);
        sayViewport();
        await refit();
        await cdp.send('Page.reload', {}, shown).catch(() => {});
        break;
      }
    }
  };

  let buf = '';
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', (chunk) => {
    buf += chunk;
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i);
      buf = buf.slice(i + 1);
      let m;
      try { m = JSON.parse(line); } catch (_) { continue; }
      act(m).catch((e) => out({ t: 'note', msg: e.message }));
    }
    if (buf.length > 1 << 20) buf = '';
  });
  // The panel went away: Chromium stays, for the agent and the next look.
  process.stdin.on('end', () => process.exit(0));
}

// ── Telling sbxw the agent is browsing ──────────────────────────────────────
// So the person sees a tab on the sandbox's terminal and can watch. Through
// the daemon's agent port, with this sandbox's token, as the relay tool does;
// the port is read from that tool, which sbxw writes with it. Best effort:
// a daemon that does not answer changes nothing here.
function agentPort() {
  if (process.env.SBXW_AGENT_PORT) return process.env.SBXW_AGENT_PORT;
  try {
    const m = fs.readFileSync(path.join(HOME, '.sbxw', 'relay.js'), 'utf8').match(/^const PORT = (\d+);/m);
    return m ? m[1] : null;
  } catch (_) {
    return null;
  }
}

async function tellSbxw(active) {
  const port = agentPort();
  let token = '';
  try { token = fs.readFileSync(path.join(HOME, '.sbxw', 'token'), 'utf8').trim(); } catch (_) { /* none */ }
  if (!port || !token) return;
  try {
    await fetch(`http://${process.env.SBXW_AGENT_HOST || 'host.docker.internal'}:${port}/api/browser/state`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: `Bearer ${token}` },
      body: JSON.stringify({ active }),
      signal: AbortSignal.timeout(1500),
    });
  } catch (_) { /* best effort */ }
}

// ── The agent's commands ────────────────────────────────────────────────────

async function withPage(fn) {
  const v = await ensure();
  await tellSbxw(true);
  const cdp = await connect(v.webSocketDebuggerUrl);
  const { targetInfos } = await cdp.send('Target.getTargets');
  let target = targetInfos.find((t) => t.type === 'page');
  if (!target) {
    const made = await cdp.send('Target.createTarget', { url: 'about:blank' });
    target = { targetId: made.targetId };
  }
  const { sessionId } = await cdp.send('Target.attachToTarget', { targetId: target.targetId, flatten: true });
  await cdp.send('Page.enable', {}, sessionId);
  const vp = readViewport();
  if (vp) await applyViewport(cdp, sessionId, vp, v);
  try {
    return await fn(cdp, sessionId);
  } finally {
    cdp.ws.close();
  }
}

async function evaluate(cdp, sid, expression) {
  const r = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }, sid);
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
  return r.result.value;
}

const where = (cdp, sid) => evaluate(cdp, sid, 'JSON.stringify({ url: location.href, title: document.title })').then(JSON.parse);

async function centre(cdp, sid, selector) {
  const box = await evaluate(cdp, sid, `(() => {
    const e = document.querySelector(${JSON.stringify(selector)});
    if (!e) return null;
    e.scrollIntoView({ block: 'center', inline: 'center' });
    const r = e.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  })()`);
  if (!box) throw new Error(`nothing matches ${selector}`);
  return box;
}

async function click(cdp, sid, selector) {
  const { x, y } = await centre(cdp, sid, selector);
  for (const type of ['mousePressed', 'mouseReleased']) {
    await cdp.send('Input.dispatchMouseEvent', { type, x, y, button: 'left', clickCount: 1 }, sid);
  }
}

const commands = {
  async start() {
    const v = await ensure();
    await tellSbxw(true);
    console.log(`${v.Browser} on ${BASE} (this sandbox only)`);
  },
  async stop() {
    const v = await version();
    await tellSbxw(false);
    if (!v) return console.log('not running');
    const cdp = await connect(v.webSocketDebuggerUrl);
    await cdp.send('Browser.close').catch(() => {});
    console.log('closed');
  },
  async open(raw) {
    const url = cleanUrl(raw);
    if (!url) throw new Error('usage: browser.mjs open <http(s) address>');
    await withPage(async (cdp, sid) => {
      const loaded = new Promise((ok) => {
        cdp.on((m, _p, s) => { if (m === 'Page.loadEventFired' && s === sid) ok(); });
        setTimeout(ok, 20000);
      });
      const r = await cdp.send('Page.navigate', { url }, sid);
      await loaded;
      const at = await where(cdp, sid);
      console.log(`${at.title || '(no title)'}\n${at.url}`);
      if (r.errorText) console.log(`load error: ${r.errorText}`);
      const text = await evaluate(cdp, sid, 'document.body ? document.body.innerText.slice(0, 300) : ""');
      const kind = blockKind(text);
      if (kind) console.log(`refused by the sandbox's network policy (${kind}): the person can allow this host from sbxw's Browser panel`);
    });
  },
  async viewport(...raw) {
    const arg = raw.join(' ').trim();
    if (!arg) {
      const vp = readViewport();
      return console.log(vp ? `${vp.name}: ${vp.w}x${vp.h}${vp.mobile ? ', mobile (touch, phone user agent)' : ''}` : 'auto: the Browser panel\'s size, else 1280x800');
    }
    const vp = arg === 'auto' ? null : parseViewport(arg);
    if (arg !== 'auto' && !vp) throw new Error('usage: browser.mjs viewport mobile | tablet | desktop | <w>x<h> [mobile] | auto');
    writeViewport(vp);
    // Laid out again now, and loaded again, so a site that picks its page
    // by user agent serves the right one.
    await withPage(async (cdp, sid) => {
      await applyViewport(cdp, sid, vp, await version());
      const loaded = new Promise((ok) => {
        cdp.on((m, _p, s) => { if (m === 'Page.loadEventFired' && s === sid) ok(); });
        setTimeout(ok, 10000);
      });
      await cdp.send('Page.reload', {}, sid);
      await loaded;
    });
    console.log(vp ? `${vp.name}: ${vp.w}x${vp.h}${vp.mobile ? ', mobile' : ''}` : 'auto');
  },
  async url() {
    await withPage(async (cdp, sid) => {
      const at = await where(cdp, sid);
      console.log(`${at.title || '(no title)'}\n${at.url}`);
    });
  },
  async text() {
    await withPage(async (cdp, sid) => console.log(await evaluate(cdp, sid, 'document.body ? document.body.innerText : ""')));
  },
  async eval(...js) {
    await withPage(async (cdp, sid) => {
      const v = await evaluate(cdp, sid, js.join(' '));
      console.log(typeof v === 'string' ? v : JSON.stringify(v, null, 2));
    });
  },
  async click(selector) {
    if (!selector) throw new Error('usage: browser.mjs click <css selector>');
    await withPage(async (cdp, sid) => { await click(cdp, sid, selector); console.log('clicked'); });
  },
  async type(selector, ...text) {
    if (!selector || !text.length) throw new Error('usage: browser.mjs type <css selector> <text>');
    await withPage(async (cdp, sid) => {
      await click(cdp, sid, selector);
      await cdp.send('Input.insertText', { text: text.join(' ') }, sid);
      console.log('typed');
    });
  },
  async shot(file) {
    if (!file) throw new Error('usage: browser.mjs shot <file.png>');
    await withPage(async (cdp, sid) => {
      const r = await cdp.send('Page.captureScreenshot', { format: 'png' }, sid);
      fs.writeFileSync(file, Buffer.from(r.data, 'base64'));
      console.log(file);
    });
  },
  serve,
};

const [cmd, ...rest] = process.argv.slice(2);
if (!commands[cmd]) {
  console.error('usage: browser.mjs start | open <url> | url | text | eval <js> | click <selector> | type <selector> <text> | shot <file.png> | viewport <mobile|tablet|desktop|WxH|auto> | stop');
  process.exit(2);
}
commands[cmd](...rest).then(
  () => { if (cmd !== 'serve') process.exit(0); },
  (e) => { console.error(`browser: ${e.message}`); process.exit(1); },
);
