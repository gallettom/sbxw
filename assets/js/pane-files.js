// ── Files and terminal panes ──────────────────────────────────────────────
//
// Two things a person does with a file around a terminal pane:
//
//  * a file path printed in the terminal is a link (Cmd/Ctrl + click) that
//    opens the Project panel's Files preview for that pane's sandbox, through
//    fxOpenPath (files.js), the way the chat's file links do;
//  * files dropped on the pane are uploaded to that pane's sandbox (the sidebar
//    Upload's route) and their paths, as the daemon answered them, are typed at
//    the pane's prompt. Never submitted: the person reads the line and presses
//    Return.
//
// The rules are the pure parts (BEGIN/END markers), which tests/js/pane-files
// .test.mjs loads on their own. The DOM part is a thin skin under them.
//
// Terminal output is DATA written by programs and agents. A path found in it
// travels as a value to fxOpenPath and is shown through textContent only: it
// never enters markup, an attribute or a URL by concatenation. Nothing is
// requested on render or hover. File names are data too: the line typed at the
// prompt is built ONLY from the daemon's answers, never from a dropped file's
// own name, and file contents are never read here.

// BEGIN pane file links
// How far one wrapped line is read around the row under the pointer, and the
// most characters taken from it: a pane printing a 10,000-character line yields
// no link rather than a slow hover.
const PANE_LINK_MAX_ROWS = 12;
const PANE_LINK_MAX_CHARS = 2000;

// The logical line (rows xterm reports as wrapped into one) holding row `y`
// (1-based, absolute in the buffer) as text and a map from each character to
// its cell: `{ text, cells: [{ y, x }] }` (x 1-based), or null when the line is
// longer than the caps above. One character per cell, wide characters once.
function paneLogicalLine(buf, y) {
  if (!buf || !buf.getLine(y - 1)) return null;
  let first = y;
  while (first > 1 && buf.getLine(first - 1)?.isWrapped) {
    if (y - first >= PANE_LINK_MAX_ROWS) return null;
    first--;
  }
  let last = y;
  while (buf.getLine(last)?.isWrapped) {
    if (last - y >= PANE_LINK_MAX_ROWS) return null;
    last++;
  }
  let text = '';
  const cells = [];
  for (let r = first; r <= last; r++) {
    const line = buf.getLine(r - 1);
    if (!line) continue;
    for (let x = 0; x < line.length; x++) {
      const cell = line.getCell(x);
      if (!cell || cell.getWidth() === 0) continue;
      const ch = cell.getChars() || ' ';
      for (let k = 0; k < ch.length; k++) cells.push({ y: r, x: x + 1 });
      text += ch;
    }
    if (text.length > PANE_LINK_MAX_CHARS * 2) return null;
  }
  return { text: text.replace(/\s+$/, ''), cells };
}

// The file paths in the logical line of row `y`: `[{ range, found, text }]`
// in xterm's link ranges (1-based, end inclusive). The detection is the
// chat's (chatFilePath): one rule for both places.
function paneFileLinksAt(buf, y, detect) {
  const line = paneLogicalLine(buf, y);
  if (!line || line.text.length > PANE_LINK_MAX_CHARS) return [];
  const out = [];
  for (const t of detect(line.text)) {
    const a = line.cells[t.start], b = line.cells[t.end - 1];
    if (!a || !b) continue;
    out.push({ range: { start: { x: a.x, y: a.y }, end: { x: b.x, y: b.y } }, found: t.found, text: line.text.slice(t.start, t.end) });
  }
  return out;
}

// What a press hands fxOpenPath. A path as printed is relative to the shell's
// current directory, which the page does not know: it is read against the
// workspace (what the Files tab does), and in a thread's worktree pane against
// that worktree (`own` threads; a shared plan's worktree has a name this page
// does not hold, a session on the main checkout works in the workspace itself).
function paneLinkTarget(path, ses, workspace) {
  if (path.startsWith('/') || path.startsWith('~/')) return path;
  if (ses.thread && ses.kind === 'own' && workspace) {
    return `${workspace.replace(/\/+$/, '')}/.sbxw-worktrees/${ses.thread}/${path.replace(/^\.\//, '')}`;
  }
  return path;
}

// The words on the hover tooltip: where a press would look, before it does.
function paneLinkTip(abs, path, ses) {
  const where = abs ? abs : `${path} (relative to the workspace, not known yet)`;
  const who = ses.thread ? `${ses.sandbox}, thread ${ses.thread}` : ses.sandbox;
  return `${where}\nin ${who}\n${PANE_LINK_GESTURE} to preview`;
}
const PANE_LINK_GESTURE = (typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || '')) ? '⌘ + click' : 'Ctrl + click';

// A xterm link provider. `env`: session() → { sandbox, mode, thread, kind } or
// null; workspace(sandbox); resolve(sandbox, target) → absolute path or null;
// open(sandbox, target); tip(text | null, event); detect(text) → tokens.
function paneLinkProvider(term, env) {
  return {
    provideLinks(y, callback) {
      const ses = env.session();
      if (!ses || !ses.sandbox) { callback(undefined); return; }
      const found = paneFileLinksAt(term.buffer.active, y, env.detect);
      if (!found.length) { callback(undefined); return; }
      callback(found.map(f => {
        const target = paneLinkTarget(f.found.path, ses, env.workspace(ses.sandbox));
        return {
          range: f.range,
          text: f.text,
          decorations: { pointerCursor: true, underline: true },
          // A plain click and a text selection keep working: only Cmd/Ctrl.
          activate(e) {
            if (!e || !(e.metaKey || e.ctrlKey)) return;
            const now = env.session();
            if (!now || now.sandbox !== ses.sandbox) return;
            env.open(ses.sandbox, target);
          },
          hover(e) { env.tip(paneLinkTip(env.resolve(ses.sandbox, target), f.found.path, ses), e); },
          leave() { env.tip(null); },
        };
      }));
    },
  };
}
// END pane file links

// BEGIN pane typing
// A path as one shell word: left bare when it only holds safe characters,
// otherwise in single quotes (a quote inside as '\'').
function paneShellQuote(p) {
  if (/^[A-Za-z0-9_\/.,:+@%=~-]+$/.test(p)) return p;
  return `'${p.replace(/'/g, `'\\''`)}'`;
}

// The line typed at the prompt for the daemon's paths: quoted, separated by
// spaces, a trailing space and NO Return. null when a path holds a control
// character (a newline typed at a prompt is a Return).
function paneTypeLine(paths) {
  if (!paths.length) return null;
  for (const p of paths) {
    if (typeof p !== 'string' || !p || /[\u0000-\u001f\u007f-\u009f\u2028\u2029\u202a-\u202e\u2066-\u2069]/.test(p)) return null;
  }
  return paths.map(paneShellQuote).join(' ') + ' ';
}

// Why `pane` cannot take keys now, as a phrase for the person, or ''. `want`:
// the session the paths were meant for ({ sandbox, mode, thread }); `env`:
// { held, shown (panes on screen), dialog (a modal is open over the panes) }.
function paneTypeBlocker(pane, want, env) {
  if (env.held) return 'terminals are held while you review a proposal';
  if (!pane || pane.index >= env.shown) return 'this pane is not on screen';
  if (pane.sandbox !== want.sandbox || pane.mode !== want.mode || (pane.thread || null) !== (want.thread || null)) {
    return 'this pane shows another session now';
  }
  if (!pane.ws || pane.ws.readyState !== 1) return 'the terminal is not connected';
  if (env.dialog) return 'a dialog is open over the terminals';
  return '';
}
// END pane typing

// ── DOM part ──────────────────────────────────────────────────────────────
const PANE_WS_OPEN = 1;

function paneEnvNow() {
  return {
    held: typeof paneInputHeld !== 'undefined' && paneInputHeld,
    shown: typeof paneCount !== 'undefined' ? paneCount : 0,
    dialog: !!document.querySelector('dialog[open]'),
  };
}

function paneSessionNow(pane) {
  if (!pane.sandbox) return null;
  const kind = pane.thread ? threadLists.get(pane.sandbox)?.kind.get(pane.thread) || 'own' : null;
  return { sandbox: pane.sandbox, mode: pane.mode, thread: pane.thread || null, kind };
}

// One tooltip for every pane, shown beside the pointer. Text only.
let paneLinkTipEl = null;
function paneLinkTipShow(text, e) {
  if (!text) { if (paneLinkTipEl) paneLinkTipEl.hidden = true; return; }
  if (!paneLinkTipEl) {
    paneLinkTipEl = document.createElement('div');
    paneLinkTipEl.className = 'pane-link-tip';
    paneLinkTipEl.setAttribute('role', 'tooltip');
    document.body.appendChild(paneLinkTipEl);
  }
  paneLinkTipEl.textContent = text;
  paneLinkTipEl.hidden = false;
  const x = e && Number.isFinite(e.clientX) ? e.clientX : 0;
  const y = e && Number.isFinite(e.clientY) ? e.clientY : 0;
  const w = paneLinkTipEl.offsetWidth, h = paneLinkTipEl.offsetHeight;
  paneLinkTipEl.style.left = `${Math.max(4, Math.min(window.innerWidth - w - 4, x + 12))}px`;
  paneLinkTipEl.style.top = `${Math.max(4, Math.min(window.innerHeight - h - 4, y + 18))}px`;
}

// Makes the paths in `term` links for `pane`. Called once per Terminal.
function paneLinksAttach(term, pane) {
  if (typeof term.registerLinkProvider !== 'function') return;
  term.registerLinkProvider(paneLinkProvider(term, {
    session: () => paneSessionNow(pane),
    workspace: sandbox => (typeof sandboxes !== 'undefined' && sandboxes.find(s => s.name === sandbox)?.workspace) || null,
    resolve: (sandbox, target) => (typeof fxResolvePath === 'function' ? fxResolvePath(sandbox, target) : null),
    open: (sandbox, target) => { if (typeof fxOpenPath === 'function') fxOpenPath(sandbox, target); },
    tip: paneLinkTipShow,
    detect: text => (typeof chatFileTokens === 'function' ? chatFileTokens(text) : []),
  }));
}

// The small notice on a pane when paths could not be typed: why, the line, and
// a Copy button. Text only.
function paneShowPathNotice(pane, reason, line) {
  pane.el.querySelector('.pane-drop-notice')?.remove();
  const box = document.createElement('div');
  box.className = 'pane-drop-notice';
  box.setAttribute('role', 'status');
  const msg = document.createElement('p');
  msg.textContent = `Uploaded, but not typed: ${reason}. The paths:`;
  const code = document.createElement('code');
  code.textContent = line;
  const row = document.createElement('div');
  row.className = 'pane-drop-row';
  const copy = document.createElement('button');
  copy.type = 'button';
  copy.textContent = 'Copy';
  copy.addEventListener('click', () => {
    const done = () => { copy.textContent = 'Copied'; };
    const fail = () => { copy.textContent = 'Copy failed: select the text'; };
    try { navigator.clipboard.writeText(line).then(done, fail); } catch (_) { fail(); }
  });
  const close = document.createElement('button');
  close.type = 'button';
  close.textContent = 'Dismiss';
  close.addEventListener('click', () => box.remove());
  row.append(copy, close);
  box.append(msg, code, row);
  pane.el.appendChild(box);
}

// Type the daemon's `paths` at `pane`'s prompt, when that pane can take keys
// for session `want`: the one function behind the pane drop and the sidebar
// Upload. No Return. Otherwise the notice with the paths and a Copy button.
// → true when typed.
function paneTypePaths(pane, want, paths) {
  const line = paneTypeLine(paths);
  if (line === null) {
    paneShowPathNotice(pane, 'a path has a control character', paths.map(p => String(p).replace(/[\u0000-\u001f\u007f-\u009f\u2028\u2029\u202a-\u202e\u2066-\u2069]/g, '?')).join(' '));
    return false;
  }
  const why = paneTypeBlocker(pane, want, paneEnvNow());
  if (why) { paneShowPathNotice(pane, why, line); return false; }
  const bytes = new TextEncoder().encode(line);
  // The socket's own hold is the last word: a frame it refuses is not typed.
  if (typeof paneFrameAllowed === 'function' && !paneFrameAllowed(bytes)) {
    paneShowPathNotice(pane, 'terminals are held while you review a proposal', line);
    return false;
  }
  pane.ws.send(bytes);
  pane.el.querySelector('.pane-drop-notice')?.remove();
  pane.term.focus();
  return true;
}

// Files dropped on a pane. Every check, the tick, the upload with progress and
// retry are the chat attach's (chatAttachTray, chatRenderTray): one rule. When
// all are uploaded their paths are typed at the prompt.
function paneDropMount(pane) {
  const veil = document.createElement('div');
  veil.className = 'pane-drop-veil';
  veil.setAttribute('aria-hidden', 'true');
  veil.hidden = true;
  const box = document.createElement('div');
  box.className = 'pane-drop-box';
  box.setAttribute('aria-live', 'polite');
  box.hidden = true;
  pane.el.append(veil, box);

  let flash = '';
  let want = null; // the session the files in the tray are for
  let finishing = false;
  let tray = null; // built on the first real drop: chat-core.js loads after this file
  const finish = () => {
    if (finishing || !tray || !tray.chips.length || tray.held) return;
    if (!tray.chips.every(c => c.state === 'done')) return;
    finishing = true;
    const paths = tray.take().map(c => c.path);
    finishing = false;
    const w = want;
    want = null;
    if (!w) return;
    if (paneTypePaths(pane, w, paths)) showToast(`📎 ${paths.join('  ')}`, 'ok');
  };
  const render = () => {
    if (typeof chatRenderTray === 'function' && tray) chatRenderTray(box, tray, flash);
    else { box.textContent = flash; }
    box.hidden = !box.childNodes.length && !box.textContent;
  };
  // The tray, built when a file is really dropped, or null when chat-core.js is absent.
  const getTray = () => {
    if (tray) return tray;
    if (typeof chatAttachTray !== 'function' || typeof chatRenderTray !== 'function') return null;
    tray = chatAttachTray({
      sandbox: () => (want ? want.sandbox : null),
      onChange: () => { render(); finish(); },
    });
    pane.dropTray = tray;
    return tray;
  };

  const hasFiles = e => [...(e.dataTransfer?.types || [])].includes('Files');
  const held = () => typeof paneInputHeld !== 'undefined' && paneInputHeld;
  let depth = 0;
  const hide = () => { depth = 0; veil.hidden = true; };
  const refusal = () => {
    if (!pane.sandbox) return 'this pane has no sandbox yet';
    const busy = tray && (tray.chips.length || tray.held);
    if (busy && want && (want.sandbox !== pane.sandbox || want.mode !== pane.mode || want.thread !== (pane.thread || null))) {
      return 'finish or remove the files already waiting in this pane first';
    }
    return '';
  };

  pane.el.addEventListener('dragenter', e => {
    if (held() || !hasFiles(e)) return;
    e.preventDefault();
    depth++;
    const why = refusal();
    veil.textContent = why ? `Cannot send here: ${why}`
      : `Drop to send to ${pane.sandbox}${pane.thread ? `\nthread ${pane.thread}` : ''}\nIts path is typed at the prompt, not submitted`;
    veil.classList.toggle('refuse', !!why);
    veil.hidden = false;
  });
  pane.el.addEventListener('dragover', e => {
    if (held() || !hasFiles(e)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = refusal() ? 'none' : 'copy';
  });
  pane.el.addEventListener('dragleave', e => { if (hasFiles(e) && --depth <= 0) hide(); });
  pane.el.addEventListener('drop', e => {
    if (held()) return; // the window's hold guard has stopped it already
    if (typeof chatDroppedFiles !== 'function') {
      hide();
      if (![...(e.dataTransfer?.types || [])].includes('Files')) return;
      e.preventDefault();
      flash = 'File drop is not available on this page';
      render();
      return;
    }
    const got = chatDroppedFiles(e.dataTransfer);
    hide();
    if (!got) return; // text, a link: as before
    e.preventDefault();
    const why = refusal();
    if (why) { flash = `Not sent: ${why}.`; render(); return; }
    const t = getTray();
    if (!t) { flash = 'File drop is not available on this page'; render(); return; }
    flash = '';
    if (!t.chips.length && !t.held) want = { sandbox: pane.sandbox, mode: pane.mode, thread: pane.thread || null };
    t.offer(got.files, got.folders);
  });
}
