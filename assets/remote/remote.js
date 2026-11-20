// ── sbxw remote: the phone page ───────────────────────────────────────────
// Wires remote-core.js to index.html. One page, one screen at a time:
// pairing, not paired, the leaders, a leader's conversation.
//
// What this page never does, on purpose:
// * keep anything of a conversation: messages, answers and dictated words
//   live in memory only, never in localStorage, sessionStorage, IndexedDB,
//   the Cache API or a console line. The one stored thing is the device's
//   name, to prefill pairing (`sbxw-remote-prefs`);
// * send before a tap: the composer sends on Send only, an answer on "Send
//   answer" only, Interrupt on its second tap; dictation fills the field;
//   a file goes up only once picked (and ticked, for a secret-looking name),
//   and reaches the leader only with a message the person sends;
// * keep audio: the wave reads levels from an AnalyserNode, nothing else;
//   the recording lives in memory until it is transcribed, then is dropped;
// * queue or retry: a request the Mac did not answer is reported, and the
//   typed text stays in the field for the person to send again;
// * hold a credential: the device token is an HttpOnly cookie;
// * approve anything: plans, merges, network requests and permission prompts
//   are not on this page; it says "This needs your Mac".
(() => {
  'use strict';
  const $ = id => document.getElementById(id);

  // ── Local preferences: the device name, nothing else ──
  const PREFS = 'sbxw-remote-prefs';
  // Two UI preferences under one key: the device's name (to prefill
  // pairing) and the dictation mode ('mac' unless the person chose 'iphone').
  const prefs = {
    read() {
      try {
        const v = JSON.parse(localStorage.getItem(PREFS) || '{}');
        return v && typeof v === 'object' ? v : {};
      } catch (_) { return {}; }
    },
    write(v) {
      const out = {};
      if (typeof v.device_name === 'string' && v.device_name) out.device_name = rcCleanName(v.device_name);
      if (v.dictation === RC_DICTATION.iphone) out.dictation = RC_DICTATION.iphone;
      try { localStorage.setItem(PREFS, JSON.stringify(out)); } catch (_) {}
    },
    name() {
      const v = prefs.read();
      return typeof v.device_name === 'string' ? rcCleanName(v.device_name) : '';
    },
    setName(name) { prefs.write({ ...prefs.read(), device_name: name }); },
    dictation() { return prefs.read().dictation === RC_DICTATION.iphone ? RC_DICTATION.iphone : RC_DICTATION.mac; },
    setDictation(mode) { prefs.write({ ...prefs.read(), dictation: mode }); },
  };

  // ── The pairing code: read from the fragment, then gone from the bar ──
  // Done first, before any request, so the code is in no history entry and
  // no later page sees it.
  let pairCode = rcPairCode(location.hash);
  if (location.hash) history.replaceState(null, '', location.pathname + location.search);

  // ── State, in memory only ──
  const st = {
    me: null,
    leaders: [],
    current: null,       // the open leader's sandbox
    items: [],           // its history, oldest first
    nextBefore: null,
    latestSeq: 0,
    loadingOlder: false,
    newer: null,         // the in-flight ?after= read
    newerAgain: false,
    drafts: new Map(),   // per leader: what was typed and not sent (memory only)
    trays: new Map(),    // per leader: the files picked for its next message
    sending: false,
    note: null,          // the status line's last outcome: { text, tone }
    hint: '',            // a quieter line (dictation's results)
    shownSeq: 0,         // rows up to here are on screen: newer ones arrive
    es: null,
    probe: null,
    paired: false,
    screen: 's-loading',
    l2: null,            // GET /r/approvals: { level2, passkey, rp_ok, counts }
    l2From: null,        // where Approvals was opened: null (leaders) or a sandbox
    item: null,          // the open detail: { what, id, data, digest }
    views: new Map(),    // per leader: 'chat' or 'kanban', and the board's place
  };

  const banner = $('unreachable');
  const api = rcClient((path, init) => fetch(path, init), {
    reach: ok => { banner.hidden = ok || !st.paired && !st.pairing; },
    unpaired: () => unpaired('This device is no longer paired', 'Pair it again from your Mac: open the pairing link or scan the QR code that sbxw shows.'),
  });

  // ── Screens ──
  const SCREENS = ['s-loading', 's-pair', 's-unpaired', 's-leaders', 's-chat', 's-approvals', 's-item', 's-passkey'];
  const BACK = ['s-chat', 's-approvals', 's-item', 's-passkey'];
  function show(id) {
    for (const s of SCREENS) $(s).hidden = s !== id;
    st.screen = id;
    $('back').hidden = !BACK.includes(id);
    $('mark').hidden = BACK.includes(id);
    $('approvals-btn').hidden = !(id === 's-leaders' || id === 's-chat');
    if (id !== 's-item') closeSheet();
    if (id !== 's-chat') { $('state').hidden = true; $('interrupt').hidden = true; disarmInterrupt(); }
    $('menu-btn').hidden = !(id === 's-leaders' || id === 's-chat');
    closeMenu();
  }
  function setTitle(title, sub) {
    $('title').textContent = title;
    $('subtitle').textContent = sub || '';
  }

  // ── Boot ──
  async function boot() {
    show('s-loading');
    const res = await api.me();
    if (res.ok && res.data?.device) return paired(res.data.device);
    if (res.status === 401 || res.status === 403) {
      // The unpaired hook drew its screen; a pairing code wins over it.
      if (pairCode) return showPairing();
      return unpaired('This device is not paired', 'Open the pairing link or scan the QR code that sbxw shows on your Mac.');
    }
    // Unreachable or broken: say so and look again in a while (a read only).
    st.pairing = !!pairCode;
    banner.hidden = false;
    setTimeout(boot, 5000);
  }

  function unpaired(title, text) {
    st.paired = false;
    stopEvents();
    st.current = null;
    st.items = [];
    st.drafts.clear();
    for (const t of st.trays.values()) t.clear();
    st.trays.clear();
    stopRecording(true);
    st.l2 = null;
    st.item = null;
    for (const v of st.views.values()) clearTimeout(v.timer);
    st.views.clear();
    stopPasskeyPoll();
    $('field').value = '';
    banner.hidden = true;
    $('unpaired-title').textContent = title;
    $('unpaired-text').textContent = text;
    setTitle('sbxw');
    show('s-unpaired');
  }

  // ── Pairing ──
  function showPairing() {
    st.pairing = true;
    setTitle('sbxw', 'Pairing');
    const field = $('pair-name');
    field.value = prefs.name() || rcDeviceName(navigator.userAgent, navigator.platform);
    $('pair-form').hidden = false;
    $('pair-wait').hidden = true;
    $('pair-end').hidden = true;
    $('pair-go').disabled = false;
    show('s-pair');
  }

  function pairEnd(state) {
    st.pairing = false;
    $('pair-form').hidden = true;
    $('pair-wait').hidden = true;
    $('pair-end').hidden = false;
    $('pair-end-title').textContent = state === 'refused' ? 'Refused' : state === 'limited' ? 'Too many attempts' : 'Not paired';
    $('pair-end-text').textContent = RC_PAIR_TEXT[state] || RC_PAIR_TEXT.failed;
  }

  $('pair-form').addEventListener('submit', async e => {
    e.preventDefault();
    const code = pairCode;
    if (!code) return pairEnd('invalid');
    const name = rcCleanName($('pair-name').value) || rcDeviceName(navigator.userAgent, navigator.platform);
    prefs.setName(name);
    $('pair-go').disabled = true;
    const res = await api.pairStart(code, name);
    // The code is single use: the daemon consumed it, whatever happens next.
    pairCode = null;
    const state = rcPairState(res, 'start');
    if (state !== 'pending') return pairEnd(state);
    const check = String(res.data.check || '');
    $('pair-check').textContent = /^\d{6}$/.test(check) ? `${check.slice(0, 3)} ${check.slice(3)}` : check;
    $('pair-text').textContent = RC_PAIR_TEXT.pending;
    $('pair-form').hidden = true;
    $('pair-wait').hidden = false;
    // The Mac says when it expired; the phone's own clock only bounds the wait
    // (the two clocks may differ), a little past the five minutes of a request.
    pollPairing(res.data.request_id, Date.now() + 6 * 60 * 1000);
  });

  async function pollPairing(id, until) {
    let misses = 0;
    for (;;) {
      await new Promise(r => setTimeout(r, 1500));
      if (Date.now() > until) return pairEnd('expired');
      const res = await api.pairStatus(id);
      const state = rcPairState(res, 'status');
      if (state === 'pending') { misses = 0; continue; }
      // A missed poll is not the end: the Mac may be busy for a moment.
      if (state === 'unreachable' && ++misses < 20) continue;
      if (state === 'paired') {
        st.pairing = false;
        const me = await api.me();
        if (me.ok && me.data?.device) return paired(me.data.device);
        return paired(res.data.device || { name: prefs.name() });
      }
      return pairEnd(state);
    }
  }

  // ── Paired ──
  async function paired(device) {
    st.paired = true;
    st.pairing = false;
    pairCode = null;
    st.me = device;
    banner.hidden = true;
    $('menu-device').textContent = typeof device.name === 'string' ? device.name : '';
    showLeaders();
    startEvents();
    const res = await api.leaders();
    if (res.ok) setLeaders(res.data);
    refreshL2();
  }

  // ── Events ──
  function startEvents() {
    stopEvents();
    if (!st.paired) return;
    const es = new EventSource('/r/events');
    st.es = es;
    es.addEventListener('open', () => { banner.hidden = true; });
    es.addEventListener('leaders', e => setLeaders(parse(e.data)));
    es.addEventListener('history', e => {
      const d = parse(e.data);
      if (d && d.sandbox === st.current) fetchNewer();
    });
    // Level 2's counts: numbers only.
    es.addEventListener('approvals', e => {
      const d = parse(e.data);
      if (d && typeof d === 'object') { st.l2 = { ...(st.l2 || { level2: true }), counts: d }; renderBadge(); }
    });
    // A board changed: re-read it if it is the one on screen.
    es.addEventListener('kanban', e => {
      const d = parse(e.data);
      if (d && d.sandbox === st.current) kbNudge(typeof d.latest === 'string' ? d.latest : '');
    });
    es.addEventListener('bye', e => {
      const d = parse(e.data);
      stopEvents();
      if (d?.reason === 'disabled') unpaired('Remote access is off', 'It was turned off on the Mac. Turn it on there, then open this page again.');
      else unpaired('This device is no longer paired', 'It was removed on the Mac. Pair it again from there.');
    });
    es.addEventListener('error', () => {
      // A closed stream, or a dropped one: ask once what happened (a 401
      // goes to the unpaired screen through the client's hook).
      stopEvents();
      scheduleProbe(2000);
    });
  }
  function stopEvents() {
    if (st.es) { st.es.close(); st.es = null; }
    if (st.probe) { clearTimeout(st.probe); st.probe = null; }
  }
  function scheduleProbe(ms) {
    if (st.probe || !st.paired) return;
    st.probe = setTimeout(async () => {
      st.probe = null;
      const res = await api.me();
      if (!st.paired) return;
      if (res.ok) {
        banner.hidden = true;
        startEvents();
        const l = await api.leaders();
        if (l.ok) setLeaders(l.data);
        if (st.current) fetchNewer();
      } else if (res.status !== 401) {
        banner.hidden = false;
        scheduleProbe(Math.min(ms * 2, 15000));
      }
    }, ms);
  }
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible' && st.paired && !st.es) scheduleProbe(10);
  });
  function parse(s) {
    try { return JSON.parse(s); } catch (_) { return null; }
  }

  // ── Leaders ──
  function setLeaders(data) {
    st.leaders = rcLeaders(data);
    renderLeaders();
    if (st.current && st.screen === 's-chat') {
      renderTail();
      renderChatTitle();
    }
  }

  function showLeaders() {
    saveDraft();
    stopRecording(false);
    st.current = null;
    disarmInterrupt();
    setTitle('Team leaders', st.me?.name ? `on ${st.me.name}` : '');
    show('s-leaders');
    renderLeaders();
  }

  function renderLeaders() {
    const ul = $('leaders');
    const now = Date.now();
    ul.replaceChildren(...st.leaders.map(l => {
      const li = document.createElement('li');
      const b = document.createElement('button');
      b.type = 'button';
      b.className = `rc-leader rc-st-${RC_RANK[l.state] !== undefined ? l.state : 'other'}`;
      const dot = document.createElement('span');
      dot.className = 'rc-dot';
      const name = document.createElement('span');
      name.className = 'rc-leader-name';
      name.textContent = l.name;
      const ago = document.createElement('span');
      ago.className = 'rc-leader-ago';
      ago.textContent = rcAgo(l.last_at, now);
      const line = document.createElement('span');
      line.className = 'rc-leader-line';
      const state = document.createElement('span');
      state.className = 'rc-leader-state';
      state.textContent = l.asks ? 'needs you' : rcStateLabel(l.state);
      line.append(state);
      if (rcActivity(l)) line.append(` · ${rcActivity(l)}`);
      b.append(dot, name, ago, line);
      b.addEventListener('click', () => openLeader(l.sandbox));
      li.append(b);
      return li;
    }));
    $('leaders-empty').hidden = st.leaders.length > 0;
  }
  setInterval(() => { if (!$('s-leaders').hidden) renderLeaders(); }, 30000);

  // ── A leader's conversation ──
  function leader() {
    return st.leaders.find(l => l.sandbox === st.current) || null;
  }
  // The bar: the leader's name, its state as a small pill, and Interrupt
  // while it works (one tap, then the confirmation strip).
  function renderChatTitle() {
    const l = leader();
    const state = !l ? 'stopped' : l.asks ? 'attention' : RC_RANK[l.state] !== undefined ? l.state : 'stopped';
    setTitle(l?.name || st.current, '');
    const pill = $('state');
    pill.textContent = !l ? 'not running' : l.asks ? 'needs you' : rcStateLabel(l.state);
    pill.className = `rc-pill rc-pill-${state}`;
    pill.hidden = false;
    const canStop = l?.state === 'working';
    $('interrupt').hidden = !canStop;
    if (!canStop && !$('confirm').hidden) disarmInterrupt();
    gateSend();
  }
  // Send waits while the leader works, while a file uploads, while the
  // recorder runs; the typed text and the files stay where they are.
  function gateSend() {
    const why = st.current ? rcSendGate(leader()) : '';
    const t = tray();
    const nothing = !field.value.trim() && !(t && t.ids().length);
    $('send').disabled = !!why || st.sending || !!(t && (t.pending() || t.held)) || rec.state !== 'idle' || nothing;
    renderStatus(why);
  }
  function renderStatus(gate = st.current ? rcSendGate(leader()) : '') {
    const t = tray();
    const s = rcStatus({
      note: st.note,
      rec: rec.state,
      gate,
      held: t?.held ? t.held.files.length : 0,
      pending: t ? t.pending() : 0,
      failed: t ? t.failed() : 0,
      files: t ? t.chips.length : 0,
      hint: st.hint,
    });
    const el = $('status');
    el.textContent = s.text;
    el.className = `rc-status rc-t-${s.tone}`;
  }

  async function openLeader(sandbox) {
    st.current = sandbox;
    st.items = [];
    st.nextBefore = null;
    st.latestSeq = 0;
    $('field').value = st.drafts.get(sandbox) || '';
    st.note = null;
    st.hint = '';
    st.shownSeq = Infinity;
    disarmInterrupt();
    renderTray();
    history.pushState({ leader: true }, '', location.pathname);
    renderChatTitle();
    $('log-top').textContent = 'Loading…';
    $('rows').replaceChildren();
    renderTail();
    show('s-chat');
    setView(view().mode);
    const res = await api.history(sandbox);
    if (st.current !== sandbox) return;
    if (!res.ok) {
      $('log-top').textContent = res.status === 404 ? 'This leader is not running any more.' : 'Could not load the conversation.';
      return;
    }
    const page = rcPage(res.data);
    st.items = rcMerge([], page.items);
    st.nextBefore = page.next_before;
    st.latestSeq = Math.max(page.latest_seq, ...st.items.map(i => Number(i.seq)));
    renderRows();
    st.shownSeq = st.latestSeq;
    scrollToEnd();
  }

  async function fetchNewer() {
    if (st.newer) { st.newerAgain = true; return; }
    const sandbox = st.current;
    const log = $('log');
    const atEnd = log.scrollHeight - log.scrollTop - log.clientHeight < 80;
    st.newer = api.history(sandbox, { after: st.latestSeq, limit: 200 });
    const res = await st.newer;
    st.newer = null;
    if (st.current !== sandbox) return;
    if (res.ok) {
      const page = rcPage(res.data);
      if (page.items.length) {
        st.items = rcMerge(st.items, page.items);
        st.latestSeq = Math.max(st.latestSeq, page.latest_seq, ...page.items.map(i => Number(i.seq) || 0));
        renderRows();
        st.shownSeq = Math.max(st.shownSeq, st.latestSeq);
        if (atEnd) scrollToEnd();
      }
    }
    if (st.newerAgain) { st.newerAgain = false; fetchNewer(); }
  }

  async function fetchOlder() {
    if (st.loadingOlder || st.nextBefore == null || !st.current) return;
    st.loadingOlder = true;
    const sandbox = st.current;
    $('log-top').textContent = 'Loading earlier messages…';
    const res = await api.history(sandbox, { before: st.nextBefore });
    st.loadingOlder = false;
    if (st.current !== sandbox) return;
    if (!res.ok) { $('log-top').textContent = 'Could not load earlier messages.'; return; }
    const page = rcPage(res.data);
    const log = $('log');
    const fromEnd = log.scrollHeight - log.scrollTop;
    st.items = rcMerge(st.items, page.items);
    st.nextBefore = page.next_before;
    renderRows();
    log.scrollTop = log.scrollHeight - fromEnd;
  }
  $('log').addEventListener('scroll', () => { if ($('log').scrollTop < 120) fetchOlder(); }, { passive: true });

  function scrollToEnd() {
    const log = $('log');
    log.scrollTop = log.scrollHeight;
  }

  // Agent markup, checked once more on the elements: only the subset's tags
  // and attributes survive, and a link must be http(s) or it becomes text.
  const TAGS = new Set(['P', 'BR', 'UL', 'OL', 'LI', 'STRONG', 'EM', 'CODE', 'PRE', 'A', 'SPAN']);
  const ATTRS = { A: ['href', 'target', 'rel', 'class'], SPAN: ['class'], OL: ['start'] };
  function agentInto(el, text) {
    el.innerHTML = rcAgentHtml(text);
    for (const node of [...el.querySelectorAll('*')]) {
      if (!TAGS.has(node.tagName)) { node.replaceWith(document.createTextNode(node.textContent)); continue; }
      const keep = ATTRS[node.tagName] || [];
      for (const a of [...node.attributes]) if (!keep.includes(a.name)) node.removeAttribute(a.name);
      if (node.tagName === 'A') {
        let ok = false;
        try { const u = new URL(node.getAttribute('href') || ''); ok = u.protocol === 'https:' || u.protocol === 'http:'; } catch (_) {}
        if (!ok) { node.replaceWith(document.createTextNode(node.textContent)); continue; }
        node.target = '_blank';
        node.rel = 'noopener noreferrer';
        node.referrerPolicy = 'no-referrer';
      }
    }
  }

  function renderRows() {
    const rows = rcRows(st.items);
    $('log-top').textContent = st.nextBefore != null ? '' : rows.length ? 'Start of the conversation' : 'No messages yet';
    $('rows').replaceChildren(...rows.map(r => {
      const el = rowEl(r);
      // Only what arrived since the last look rises in; a re-render is still.
      if (r.seq > st.shownSeq && (r.kind === 'agent' || r.kind === 'me')) el.classList.add('rc-new');
      return el;
    }));
  }

  function rowEl(r) {
    const d = document.createElement('div');
    if (r.kind === 'agent') {
      d.className = 'rc-msg rc-agent';
      if (r.from) {
        const f = document.createElement('div');
        f.className = 'rc-from';
        f.textContent = `From ${r.from}`;
        d.append(f);
      }
      const body = document.createElement('div');
      body.className = 'rc-md';
      agentInto(body, r.text);
      d.append(body);
    } else if (r.kind === 'me') {
      d.className = 'rc-msg rc-me';
      d.textContent = r.text;
    } else if (r.kind === 'sbxw') {
      d.className = 'rc-sbxw';
      const b = document.createElement('b');
      b.textContent = 'From sbxw';
      d.append(b, document.createElement('br'), r.text);
    } else if (r.kind === 'steps') {
      d.className = 'rc-steps';
      d.append(document.createTextNode(r.count === 1 ? '1 step' : `${r.count} steps`));
    } else if (r.kind === 'asked') {
      d.className = 'rc-note';
      d.textContent = `Asked: ${r.text}`;
    } else if (r.kind === 'mac') {
      d.className = 'rc-mac-line';
      d.textContent = 'Asked to allow a tool. This needs your Mac.';
    } else {
      d.className = 'rc-note';
      d.textContent = r.text;
    }
    return d;
  }

  // ── The tail: working, a question, or "This needs your Mac" ──
  function renderTail() {
    const tail = $('tail');
    const l = leader();
    const pending = rcPending(l);
    if (pending?.kind === 'question') return tail.replaceChildren(questionCard(l, pending.steps));
    if (pending?.kind === 'mac') {
      const card = document.createElement('div');
      card.className = 'rc-card-q';
      const p = document.createElement('p');
      p.textContent = pending.text;
      const hint = document.createElement('p');
      hint.className = 'rc-q-hint';
      hint.textContent = 'This needs your Mac.';
      card.append(p, hint);
      return tail.replaceChildren(card);
    }
    if (l?.state === 'working') {
      const w = document.createElement('div');
      w.className = 'rc-working';
      const dot = document.createElement('span');
      dot.className = 'rc-pulse';
      const t = document.createElement('span');
      t.textContent = rcActivity(l) ? `Working · ${rcActivity(l)}` : 'Working…';
      w.append(dot, t);
      return tail.replaceChildren(w);
    }
    tail.replaceChildren();
  }

  // The same question re-rendered by a `leaders` event keeps what was picked.
  let picks = { key: '', picked: [] };
  function questionCard(l, steps) {
    const key = `${l.sandbox}\n${JSON.stringify(steps)}`;
    if (picks.key !== key) picks = { key, picked: steps.map(() => null) };
    const card = document.createElement('div');
    card.className = 'rc-card-q';
    const submit = document.createElement('button');
    submit.type = 'button';
    submit.className = 'rc-btn rc-primary';
    submit.textContent = 'Send answer';
    const status = document.createElement('p');
    status.className = 'rc-q-hint';
    status.hidden = true;
    const ready = () => { submit.disabled = picks.picked.includes(null); };
    steps.forEach((step, si) => {
      const p = document.createElement('p');
      p.textContent = step.text;
      const opts = document.createElement('div');
      opts.className = 'rc-opts';
      step.options.forEach((label, oi) => {
        const b = document.createElement('button');
        b.type = 'button';
        b.className = 'rc-opt';
        b.textContent = String(label);
        b.classList.toggle('rc-picked', picks.picked[si] === oi);
        b.addEventListener('click', () => {
          picks.picked[si] = oi;
          opts.querySelectorAll('.rc-opt').forEach((x, xi) => x.classList.toggle('rc-picked', xi === oi));
          ready();
        });
        opts.append(b);
      });
      card.append(p, opts);
    });
    submit.addEventListener('click', async () => {
      if (submit.disabled) return;
      const sandbox = l.sandbox;
      card.querySelectorAll('button').forEach(b => { b.disabled = true; });
      const res = await api.answer(sandbox, picks.picked.map(i => i + 1));
      if (res.ok) {
        status.textContent = 'Answer sent.';
        status.hidden = false;
        submit.hidden = true;
        return;
      }
      status.textContent = rcFailText('Answer', res);
      status.hidden = false;
      card.querySelectorAll('button').forEach(b => { b.disabled = false; });
      ready();
    });
    ready();
    card.append(submit, status);
    return card;
  }

  // ── Composer ──
  const field = $('field');
  // The status line's outcome: a refusal (err), a warning (warn), "Sent." (ok).
  function note(text, tone = 'err') {
    st.note = text ? { text, tone } : null;
    renderStatus();
  }
  // Warn in place about what the daemon will refuse; Send stays the person's.
  field.addEventListener('input', () => {
    if (!st.sending) {
      const why = field.value.trim() ? rcMessageCheck(field.value) : null;
      // The typing check's own warning comes and goes with the text; a
      // refusal from the daemon stays until the next action.
      const typed = st.note && (st.note.text === RC_MSG_REASON.command || st.note.text === RC_MSG_REASON.control);
      if (why === 'command' || why === 'control') note(RC_MSG_REASON[why], 'warn');
      else if (typed) note('');
    }
    gateSend();
  });
  function saveDraft() {
    if (st.current) {
      if (field.value) st.drafts.set(st.current, field.value);
      else st.drafts.delete(st.current);
    }
  }
  // Where `field-sizing` is not supported, grow the box by hand (CSSOM, not
  // a style attribute in markup: allowed under the page's CSP).
  if (!(window.CSS && CSS.supports('field-sizing', 'content'))) {
    field.addEventListener('input', () => {
      field.style.height = 'auto';
      field.style.height = `${Math.min(field.scrollHeight + 2, 160)}px`;
    });
  }
  $('composer').addEventListener('submit', e => {
    e.preventDefault();
    // Only Send submits: a textarea's Enter is a new line, never a send.
  });
  const sendLabel = $('send-label');
  $('send').addEventListener('click', async () => {
    const sandbox = st.current;
    const raw = field.value;
    const text = raw.trim();
    const t = tray();
    const ids = t ? t.ids() : [];
    if (!sandbox || (!text && !ids.length) || st.sending || rcSendGate(leader())) return;
    if (t && (t.pending() || t.held) || rec.state !== 'idle') return;
    // A message takes 1 to 4 s (the daemon waits for the echo before the
    // Return): the composer says so, and a second tap does nothing.
    st.sending = true;
    const send = $('send');
    send.disabled = true;
    send.setAttribute('aria-busy', 'true');
    sendLabel.textContent = 'Sending…';
    $('composer').setAttribute('aria-busy', 'true');
    note('');
    // The ids, never a path: the daemon adds the paths itself.
    const res = await api.message(sandbox, text, ids);
    st.sending = false;
    send.removeAttribute('aria-busy');
    sendLabel.textContent = 'Send';
    $('composer').removeAttribute('aria-busy');
    // Refused or not submitted: the text and the files stay, nothing is resent.
    if (!res.ok) { note(rcFailText('Message', res), res.data?.reason === 'not_submitted' ? 'warn' : 'err'); gateSend(); return; }
    if (t) t.sent(ids);
    // Sent: what was sent leaves the field; anything typed meanwhile stays.
    if (st.current === sandbox) {
      if (field.value === raw) field.value = '';
      else if (field.value.startsWith(raw)) field.value = field.value.slice(raw.length).replace(/^\s+/, '');
      field.dispatchEvent(new Event('input'));
      st.hint = '';
      note('Sent.', 'ok');
      setTimeout(() => { if (st.note?.text === 'Sent.') note(''); }, 2500);
    } else {
      st.drafts.delete(sandbox);
    }
    gateSend();
  });

  // ── Interrupt: in the bar while the leader works, then a second tap ──
  let armTimer = null;
  function disarmInterrupt() {
    clearTimeout(armTimer);
    $('interrupt').classList.remove('rc-armed');
    $('interrupt').setAttribute('aria-expanded', 'false');
    $('confirm').hidden = true;
  }
  $('interrupt').addEventListener('click', () => {
    if (!st.current) return;
    if (!$('confirm').hidden) { disarmInterrupt(); return; }
    const l = leader();
    $('confirm-text').textContent = `Interrupt ${l?.name || st.current}?`;
    $('interrupt').classList.add('rc-armed');
    $('interrupt').setAttribute('aria-expanded', 'true');
    $('confirm').hidden = false;
    armTimer = setTimeout(disarmInterrupt, 6000);
  });
  $('interrupt-go').addEventListener('click', async () => {
    const sandbox = st.current;
    if (!sandbox || $('confirm').hidden) return;
    disarmInterrupt();
    $('interrupt').disabled = true;
    const res = await api.interrupt(sandbox);
    $('interrupt').disabled = false;
    note(res.ok ? 'Interrupt sent.' : rcFailText('Interrupt', res), res.ok ? 'ok' : 'err');
  });
  $('interrupt-cancel').addEventListener('click', disarmInterrupt);

  // ── Files: picked, checked, uploaded one at a time, sent with a message ──
  function tray(sandbox = st.current) {
    if (!sandbox) return null;
    let t = st.trays.get(sandbox);
    if (!t) {
      t = rcTray({
        upload: (file, onProgress, signal) => rcUploadXhr(XMLHttpRequest, sandbox, file, onProgress, signal),
        // A file removed before sending leaves the Mac too (best effort:
        // unsent files expire there by themselves).
        drop: id => { api.dropUpload(sandbox, id); },
        onChange: () => { if (st.current === sandbox) renderTray(); },
      });
      st.trays.set(sandbox, t);
    }
    return t;
  }
  const fileInput = $('file');
  $('attach').addEventListener('click', () => {
    if (!st.current || rec.state !== 'idle') return;
    fileInput.value = '';
    fileInput.click();
  });
  fileInput.addEventListener('change', () => {
    const t = tray();
    const files = [...(fileInput.files || [])];
    fileInput.value = '';
    if (!t || !files.length) return;
    if (st.note?.tone !== 'warn') note('');
    $('tick').checked = false;
    t.offer(files);
  });
  $('held-go').addEventListener('click', () => { const t = tray(); if (t) t.confirmHeld($('tick').checked === true); });
  $('held-cancel').addEventListener('click', () => { const t = tray(); if (t) { t.note = ''; t.discardHeld(); } });

  const ICON_FILE = 'M7 3.5h6.5L18 8v11.5a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1v-15a1 1 0 0 1 1-1zM13.5 3.5V8H18';
  const ICON_X = 'M7 7l10 10M17 7 7 17';
  const ICON_RETRY = 'M19 12a7 7 0 1 1-2.05-4.95M19 4.5V8h-3.5';
  const SVG = 'http://www.w3.org/2000/svg';
  function icon(d) {
    const svg = document.createElementNS(SVG, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('aria-hidden', 'true');
    const path = document.createElementNS(SVG, 'path');
    path.setAttribute('d', d);
    svg.append(path);
    return svg;
  }
  function iconButton(d, label, onClick, cls = '') {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = `rc-icon-btn ${cls}`.trim();
    b.setAttribute('aria-label', label);
    b.append(icon(d));
    b.addEventListener('click', onClick);
    return b;
  }
  function renderTray() {
    const t = tray();
    const box = $('tray');
    const ul = $('chips');
    if (!t) { box.hidden = true; ul.replaceChildren(); return; }
    // The held files, waiting for the tick: nothing of them is sent yet.
    const held = $('held');
    held.hidden = !t.held;
    if (t.held) $('held-why').textContent = t.held.reasons.join('\n');
    ul.replaceChildren(...t.chips.map(c => {
      const li = document.createElement('li');
      li.className = `rc-fchip rc-f-${c.state}`;
      li.dataset.state = c.state;
      const ic = document.createElement('span');
      ic.className = 'rc-fchip-icon';
      ic.append(icon(ICON_FILE));
      const tx = document.createElement('span');
      tx.className = 'rc-fchip-text';
      const name = document.createElement('span');
      name.className = 'rc-fchip-name';
      name.textContent = chatShortName(c.name);
      const meta = document.createElement('span');
      meta.className = 'rc-fchip-meta';
      meta.textContent = c.state === 'sending' ? `${chatSize(c.size)} · ${c.pct < 1 ? `${Math.round(c.pct * 100)} %` : 'the Mac checks it'}`
        : c.state === 'queued' ? `${chatSize(c.size)} · waiting`
          : c.state === 'done' ? `${chatSize(c.size)} · ready`
            : c.error;
      tx.append(name, meta);
      const acts = document.createElement('span');
      acts.className = 'rc-fchip-acts';
      if (c.state === 'failed') acts.append(iconButton(ICON_RETRY, `Retry ${chatShortName(c.name)}`, () => t.retry(c.key), 'rc-retry'));
      acts.append(iconButton(ICON_X, `Remove ${chatShortName(c.name)}`, () => t.remove(c.key), 'rc-remove'));
      li.append(ic, tx, acts);
      if (c.state === 'sending' || c.state === 'queued') {
        const bar = document.createElement('span');
        bar.className = 'rc-fchip-bar';
        bar.setAttribute('role', 'progressbar');
        bar.setAttribute('aria-label', `Uploading ${chatShortName(c.name)}`);
        bar.setAttribute('aria-valuemin', '0');
        bar.setAttribute('aria-valuemax', '100');
        bar.setAttribute('aria-valuenow', String(Math.round(c.pct * 100)));
        const fill = document.createElement('i');
        fill.style.width = `${Math.round(c.pct * 100)}%`; // CSSOM: allowed by the CSP
        bar.append(fill);
        li.append(bar);
      }
      return li;
    }));
    box.hidden = !t.held && !t.chips.length;
    // A check's refusal ("Not attached: ...") is said once, in the status line.
    if (t.note) { const n = t.note; t.note = ''; note(n, 'err'); }
    gateSend();
  }

  // ── Dictation: a live wave while it records, then into the field ──
  const mic = $('mic');
  const composer = $('composer');
  const rec = { state: 'idle', stream: null, recorder: null, chunks: [], timer: null, started: 0, audio: null, analyser: null, frame: 0, levels: [], mode: 'pulse', discard: false };
  const micWhy = !window.isSecureContext
    ? 'Dictation needs the HTTPS address of sbxw.'
    : !(navigator.mediaDevices && navigator.mediaDevices.getUserMedia) || typeof MediaRecorder === 'undefined'
      ? 'This browser cannot record audio.'
      : '';
  if (micWhy) {
    mic.classList.add('rc-off');
    mic.setAttribute('aria-label', `Dictate (unavailable: ${micWhy})`);
  }
  const reducedMotion = window.matchMedia ? window.matchMedia('(prefers-reduced-motion: reduce)') : null;
  function setRec(state) {
    rec.state = state;
    composer.dataset.rec = state;
    $('rec').hidden = state === 'idle';
    mic.setAttribute('aria-label', state === 'recording' ? 'Stop dictation' : micWhy ? `Dictate (unavailable: ${micWhy})` : 'Dictate');
    mic.setAttribute('aria-pressed', String(state === 'recording'));
    mic.disabled = state === 'transcribing';
    gateSend();
  }
  mic.addEventListener('click', () => {
    if (rec.state === 'listening') { speech.stop(); return; }
    if (rec.state === 'transcribing' || rec.starting) return;
    if (rec.recorder) { rec.recorder.stop(); return; }
    // iPhone dictation, when chosen and available at this tap; else the Mac.
    if (prefs.dictation() === RC_DICTATION.iphone && !rec.recorder && speech.start()) return;
    macRecord();
  });
  async function macRecord() {
    if (micWhy) { st.hint = micWhy; renderStatus(); return; }
    rec.starting = true;
    let stream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
    } catch (e) {
      rec.starting = false;
      st.hint = e && e.name === 'NotAllowedError'
        ? 'Microphone refused. Allow it for this site in Settings, then try again.'
        : e && e.name === 'NotFoundError' ? 'No microphone found.' : 'The microphone could not start.';
      renderStatus();
      return;
    }
    rec.starting = false;
    if (!st.current || document.visibilityState === 'hidden') { stream.getTracks().forEach(t => t.stop()); return; }
    const recorder = new MediaRecorder(stream);
    Object.assign(rec, { stream, recorder, chunks: [], started: Date.now(), discard: false, sandbox: st.current });
    recorder.addEventListener('dataavailable', e => { if (e.data && e.data.size) rec.chunks.push(e.data); });
    recorder.addEventListener('stop', () => finishRecording(recorder.mimeType));
    recorder.start();
    st.hint = '';
    // A warning stays, and so does the sentence that says why the Mac records.
    if (st.note?.tone !== 'warn' && st.note?.text !== RC_SPEECH_FALLBACK_TEXT) st.note = null;
    startWave(stream);
    setRec('recording');
    const tick = () => {
      const s = Math.floor((Date.now() - rec.started) / 1000);
      $('rec-time').textContent = rcClock(s);
      if (s >= RC_VOICE_MAX_S && rec.recorder) rec.recorder.stop();
    };
    tick();
    rec.timer = setInterval(tick, 250);
  }

  // ── iPhone dictation (Web Speech API), chosen in the menu ──
  // The page opens no audio stream of its own here: no wave, a steady dot
  // and the time. The final words go into the field at the caret; interim
  // ones are shown lighter while they come. A refusal of the browser before
  // any word falls back to the Mac for this recording, with one sentence.
  const lastCaret = { value: null, at: 0 };
  field.addEventListener('blur', () => { lastCaret.value = field.value; lastCaret.at = field.selectionStart; });
  const speech = {
    r: null, base: '', caret: 0, final: '', heard: false, sandbox: null, fellBack: false,
    start() {
      const SR = window.SpeechRecognition || window.webkitSpeechRecognition;
      if (typeof SR !== 'function') return speech.fallback();
      let r;
      try {
        r = new SR();
        r.lang = rcSpeechLang(navigator.language, document.documentElement.lang);
        r.interimResults = true;
        r.continuous = true;
        r.maxAlternatives = 1;
      } catch (_) { return speech.fallback(); }
      // The caret: where it was when the field lost focus to the mic button,
      // if the text has not changed since; else the end.
      const caret = document.activeElement === field ? field.selectionStart : lastCaret.value === field.value ? lastCaret.at : field.value.length;
      Object.assign(speech, { r, base: field.value, caret, final: '', heard: false, sandbox: st.current, fellBack: false });
      r.onresult = e => {
        if (speech.r !== r) return;
        speech.heard = true;
        const t = rcSpeechText(e.results);
        speech.final = t.final;
        // The field holds what is final, at the caret; the bar shows both.
        if (st.current === speech.sandbox) field.value = rcInsertAt(speech.base, speech.caret, t.final).value;
        const tail = t.final.length > 140 ? `…${t.final.slice(-140)}` : t.final;
        $('rec-final').textContent = tail;
        $('rec-interim').textContent = t.interim;
      };
      r.onerror = e => {
        if (speech.r !== r) return;
        if (!speech.heard && rcSpeechFallback(e && e.error)) {
          speech.fellBack = true;
          speech.end();
          note(RC_SPEECH_FALLBACK_TEXT, 'info');
          macRecord();
        }
        // Any other error (no-speech, aborted...): onend follows and says it.
      };
      r.onend = () => { if (speech.r === r) speech.end(); };
      try {
        r.start();
      } catch (_) {
        speech.r = null;
        return speech.fallback();
      }
      $('rec-final').textContent = '';
      $('rec-interim').textContent = '';
      $('rec-words').hidden = false;
      $('rec-time').textContent = rcClock(0);
      rec.started = Date.now();
      st.hint = '';
      if (st.note?.tone !== 'warn') st.note = null;
      composer.dataset.wave = 'none';
      setRec('listening');
      clearInterval(rec.timer);
      rec.timer = setInterval(() => {
        const s = Math.floor((Date.now() - rec.started) / 1000);
        $('rec-time').textContent = rcClock(s);
        if (s >= RC_VOICE_MAX_S) speech.stop();
      }, 250);
      return true;
    },
    // Not available at this tap: say so once, and let the Mac record.
    fallback() {
      note(RC_SPEECH_FALLBACK_TEXT, 'info');
      return false;
    },
    stop() { try { speech.r?.stop(); } catch (_) { speech.end(); } },
    abort() { try { speech.r?.abort(); } catch (_) {} speech.end(); },
    end() {
      const r = speech.r;
      if (!r) return;
      speech.r = null;
      r.onresult = r.onerror = r.onend = null;
      clearInterval(rec.timer);
      $('rec-words').hidden = true;
      $('rec-final').textContent = '';
      $('rec-interim').textContent = '';
      setRec('idle');
      if (speech.fellBack) return;
      const words = speech.final;
      if (st.current === speech.sandbox) {
        const put = rcInsertAt(speech.base, speech.caret, words);
        field.value = put.value;
        // The advisory checks run on dictated text as on typed text.
        field.dispatchEvent(new Event('input'));
        st.hint = words ? 'Added to the field: read it, then Send.' : 'No words heard.';
        renderStatus();
      } else if (speech.sandbox && words) {
        st.drafts.set(speech.sandbox, rcInsertAt(st.drafts.get(speech.sandbox) || speech.base, (st.drafts.get(speech.sandbox) || speech.base).length, words).value);
      }
    },
  };
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'hidden' && speech.r) speech.stop(); });

  // The wave: levels from an AnalyserNode on the same stream, drawn on a
  // canvas (no data: or blob: URL). Recording never depends on it: without
  // an analyser, a steady pulse; with reduced motion, a plain level meter.
  function startWave(stream) {
    let analyser = null;
    try {
      const Ctx = window.AudioContext || window.webkitAudioContext;
      if (Ctx) {
        rec.audio = new Ctx();
        const src = rec.audio.createMediaStreamSource(stream);
        analyser = rec.audio.createAnalyser();
        analyser.fftSize = 1024;
        analyser.smoothingTimeConstant = 0.6;
        src.connect(analyser);
        rec.audio.resume?.().catch(() => {});
      }
    } catch (_) {
      analyser = null;
    }
    rec.analyser = analyser;
    rec.levels = [];
    rec.mode = rcWaveMode(!!analyser, !!reducedMotion?.matches);
    composer.dataset.wave = rec.mode;
    $('rec').dataset.level = '0';
    if (!analyser) return;
    const buf = new Uint8Array(analyser.fftSize);
    let last = 0;
    const step = now => {
      if (!rec.analyser) return;
      rec.frame = requestAnimationFrame(step);
      // About 30 levels a second for the bars; 8 for the plain meter.
      if (now - last < (rec.mode === 'meter' ? 125 : 33)) return;
      last = now;
      rec.analyser.getByteTimeDomainData(buf);
      const level = rcLevel(buf);
      rec.levels.push(level);
      if (rec.levels.length > 240) rec.levels.splice(0, rec.levels.length - 240);
      $('rec').dataset.level = level.toFixed(2);
      drawWave();
    };
    rec.frame = requestAnimationFrame(step);
  }
  function drawWave(dim) {
    const c = $('wave');
    const dpr = Math.min(3, window.devicePixelRatio || 1);
    const w = Math.max(1, Math.round(c.clientWidth * dpr));
    const h = Math.max(1, Math.round(c.clientHeight * dpr));
    if (c.width !== w || c.height !== h) { c.width = w; c.height = h; }
    const g = c.getContext('2d');
    if (!g) return;
    g.clearRect(0, 0, w, h);
    g.fillStyle = dim ? '#6e7681' : '#3fb950';
    const mid = h / 2;
    if (rec.mode === 'meter') {
      // A level meter: one bar from the left, no scrolling.
      const v = rec.levels.at(-1) || 0;
      g.globalAlpha = 0.25;
      g.fillRect(0, mid - 2 * dpr, w, 4 * dpr);
      g.globalAlpha = 1;
      g.fillRect(0, mid - 2 * dpr, Math.max(4 * dpr, v * w), 4 * dpr);
      return;
    }
    // Bars, newest at the right, flat (a dot) when silent.
    const bar = 3 * dpr, gap = 2 * dpr, n = Math.floor(w / (bar + gap));
    const lv = rec.levels.slice(-n);
    for (let i = 0; i < n; i++) {
      const v = lv[lv.length - n + i] ?? 0;
      const bh = Math.max(bar, v * h * 0.95);
      const x = w - (n - i) * (bar + gap) + gap;
      g.globalAlpha = 0.35 + 0.65 * Math.min(1, v * 1.6);
      g.beginPath();
      if (g.roundRect) g.roundRect(x, mid - bh / 2, bar, bh, bar / 2); else g.rect(x, mid - bh / 2, bar, bh);
      g.fill();
    }
    g.globalAlpha = 1;
  }
  // Every end of a recording: the analyser stops, the AudioContext closes,
  // the microphone's tracks stop.
  function stopWave() {
    cancelAnimationFrame(rec.frame);
    rec.analyser = null;
    const a = rec.audio;
    rec.audio = null;
    if (a && a.state !== 'closed') a.close().catch(() => {});
  }
  // `discard`: the page hid or the device was forgotten: stop and keep nothing.
  function stopRecording(discard) {
    if (speech.r) { if (discard) speech.abort(); else speech.stop(); }
    if (!rec.recorder) return;
    rec.discard = !!discard;
    try { rec.recorder.stop(); } catch (_) { finishRecording(''); }
  }
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden' && rec.recorder) stopRecording(true);
  });
  window.addEventListener('pagehide', () => stopRecording(true));

  async function finishRecording(type) {
    clearInterval(rec.timer);
    stopWave();
    rec.stream?.getTracks().forEach(t => t.stop());
    const blob = new Blob(rec.chunks, { type: type || 'audio/mp4' });
    const discard = rec.discard;
    const sandbox = rec.sandbox;
    Object.assign(rec, { stream: null, recorder: null, chunks: [], discard: false });
    if (discard) {
      setRec('idle');
      st.hint = 'Recording stopped: the page was hidden. Nothing was sent.';
      renderStatus();
      return;
    }
    drawWave(true);
    setRec('transcribing');
    let res;
    try {
      res = await api.transcribe(await toPcm(blob));
    } catch (_) {
      res = null;
    }
    setRec('idle');
    if (st.note?.text === RC_SPEECH_FALLBACK_TEXT) st.note = null;
    const say = text => { st.hint = text; renderStatus(); };
    if (!res) return say('This recording could not be read.');
    if (!res.ok) return say(res.status === 503 ? 'Dictation is not installed on the Mac.' : rcFailText('Recording', res).replace('not sent', 'not transcribed'));
    const words = typeof res.data?.text === 'string' ? res.data.text : '';
    if (!words.trim()) return say('No words heard.');
    if (st.current === sandbox) {
      field.value = rcWithDictation(field.value, words);
      field.dispatchEvent(new Event('input'));
      say('Added to the field: read it, then Send.');
    } else if (sandbox) {
      st.drafts.set(sandbox, rcWithDictation(st.drafts.get(sandbox) || '', words));
    }
  }

  // Whatever the recorder produced, as 16 kHz mono f32 (the daemon parses no
  // audio format). The audio is never kept.
  async function toPcm(blob) {
    const Ctx = window.AudioContext || window.webkitAudioContext;
    const ctx = new Ctx();
    let decoded;
    try { decoded = await ctx.decodeAudioData(await blob.arrayBuffer()); } finally { ctx.close(); }
    const frames = Math.max(1, Math.ceil(decoded.duration * RC_VOICE_RATE));
    const off = new OfflineAudioContext(1, frames, RC_VOICE_RATE);
    const src = off.createBufferSource();
    src.buffer = decoded;
    src.connect(off.destination);
    src.start();
    return (await off.startRendering()).getChannelData(0);
  }

  // ── Menu: the device, and forgetting it ──
  function closeMenu() {
    $('menu').hidden = true;
    $('menu-btn').setAttribute('aria-expanded', 'false');
    const f = $('forget');
    f.classList.remove('rc-armed');
    f.textContent = 'Forget this device';
  }
  // Dictation: Mac (private) or iPhone; a UI preference, kept on the phone.
  function renderDictation() {
    const mode = prefs.dictation();
    for (const b of document.querySelectorAll('#dict .rc-seg-opt')) b.setAttribute('aria-checked', String(b.dataset.mode === mode));
    const lang = $('dict-lang');
    lang.hidden = mode !== RC_DICTATION.iphone;
    lang.textContent = `Language: ${rcSpeechLang(navigator.language, document.documentElement.lang)} (this browser's).`;
  }
  for (const b of document.querySelectorAll('#dict .rc-seg-opt')) {
    b.addEventListener('click', e => {
      e.stopPropagation();
      if (rec.state !== 'idle') return;
      prefs.setDictation(b.dataset.mode === RC_DICTATION.iphone ? RC_DICTATION.iphone : RC_DICTATION.mac);
      renderDictation();
    });
  }
  renderDictation();
  $('menu-btn').addEventListener('click', e => {
    e.stopPropagation();
    const open = $('menu').hidden;
    closeMenu();
    $('menu').hidden = !open;
    $('menu-btn').setAttribute('aria-expanded', String(open));
  });
  document.addEventListener('click', e => { if (!$('menu').hidden && !$('menu').contains(e.target)) closeMenu(); });
  $('forget').addEventListener('click', async () => {
    const f = $('forget');
    if (!f.classList.contains('rc-armed')) {
      f.classList.add('rc-armed');
      f.textContent = 'Tap again to forget it';
      return;
    }
    f.disabled = true;
    const res = await api.logout();
    f.disabled = false;
    if (res.ok || res.status === 401) {
      unpaired('This device was forgotten', 'To use it again, pair it from your Mac.');
    } else {
      f.classList.remove('rc-armed');
      f.textContent = 'Forget this device';
      $('menu-device').textContent = rcFailText('Request', res).replace('Request not sent', 'Not forgotten');
    }
  });

  // ══ The Kanban: a leader's board, read only ═══════════════════════════
  // Conversation | Kanban under the bar. The board is the route's answer
  // drawn as it comes: one column at a time on a phone (a picker with the
  // counts, or a swipe), all five side by side from 700 px. Nothing on it
  // writes: no add, move, edit or delete. The selected column, each
  // column's scroll and an open ticket survive a re-read.
  function view(sandbox = st.current) {
    let v = st.views.get(sandbox);
    if (!v) { v = { mode: 'chat', board: null, col: null, scroll: {}, ticket: null, logTop: null, loading: false, again: false, timer: null }; st.views.set(sandbox, v); }
    return v;
  }
  function setView(mode) {
    const v = view();
    const log = $('log');
    if (mode === 'kanban' && v.mode === 'chat' && !log.hidden) v.logTop = log.scrollTop;
    v.mode = mode;
    const k = mode === 'kanban';
    $('v-chat').setAttribute('aria-selected', String(!k));
    $('v-kanban').setAttribute('aria-selected', String(k));
    $('board').hidden = !k;
    log.hidden = k;
    $('composer').hidden = k;
    if (k) {
      saveDraft();
      stopRecording(false);
      if (v.board) renderBoard(); else $('kb-lanes').replaceChildren();
      loadBoard();
    } else if (v.logTop !== null) {
      log.scrollTop = v.logTop;
    }
  }
  $('v-chat').addEventListener('click', () => setView('chat'));
  $('v-kanban').addEventListener('click', () => setView('kanban'));

  function kbStatus(text) {
    $('kb-status').textContent = text;
    $('kb-status').hidden = !text;
  }
  async function loadBoard() {
    const sandbox = st.current;
    const v = view(sandbox);
    if (v.loading) { v.again = true; return; }
    v.loading = true;
    if (!v.board) kbStatus('Loading the board…');
    const res = await api.get(rcKbPaths(sandbox));
    v.loading = false;
    if (st.current !== sandbox) return;
    if (!res.ok) {
      kbStatus(res.status === 404 ? 'This leader is no longer running.' : rcFailText('Board', res).replace('Board not sent', 'The board could not be read'));
      if (res.status === 404) { v.board = null; $('kb-lanes').replaceChildren(); $('kb-pick').replaceChildren(); }
    } else {
      kbStatus('');
      const board = rcKbBoard(res.data);
      const same = v.board && v.board.latest && v.board.latest === board.latest;
      if (!same) {
        v.board = board;
        if (!v.col || !board.columns.some(c => c.id === v.col)) v.col = rcKbFirst(board);
        if (view().mode === 'kanban') renderBoard();
      }
      if (v.ticket && view().mode === 'kanban') {
        const key = v.ticket.card.key;
        const fresh = v.board?.columns.flatMap(c => c.cards).find(c => c.key === key);
        openTicket(fresh || v.ticket.card, true);
      }
    }
    // A nudge that came during this read wants a newer board, unless this
    // answer already is the one it named.
    if (v.again) {
      v.again = false;
      if (!(v.want && v.board?.latest === v.want)) loadBoard();
    }
  }
  // The `kanban` event: a short wait gathers a burst into one read.
  function kbNudge(latest) {
    const v = view();
    if (v.board && latest && v.board.latest === latest) return;
    v.want = latest;
    clearTimeout(v.timer);
    v.timer = setTimeout(() => { if (view().mode === 'kanban') loadBoard(); else v.board = null; }, 400);
  }

  const wide = window.matchMedia ? window.matchMedia('(min-width: 700px)') : { matches: false };
  function renderBoard() {
    const v = view();
    const b = v.board;
    if (!b) return;
    const lanes = $('kb-lanes');
    // Keep each column's scroll across a redraw.
    for (const lane of lanes.querySelectorAll('.rc-kb-lane')) v.scroll[lane.dataset.col] = lane.querySelector('.rc-kb-list').scrollTop;
    $('kb-pick').replaceChildren(...b.columns.map(c => {
      const t = el('button', `rc-kb-tab rc-tone-${c.tone}`);
      t.type = 'button';
      t.setAttribute('role', 'tab');
      t.setAttribute('aria-selected', String(c.id === v.col));
      t.dataset.col = c.id;
      t.append(el('span', null, c.title), el('span', 'rc-kb-count', c.count));
      t.addEventListener('click', () => pickCol(c.id));
      return t;
    }));
    lanes.replaceChildren(...b.columns.map(c => {
      const lane = el('section', `rc-kb-lane rc-tone-${c.tone}${c.id === v.col ? ' rc-on' : ''}`);
      lane.dataset.col = c.id;
      const h = el('h3', 'rc-kb-h');
      h.append(el('span', null, c.title), el('span', 'rc-kb-count', c.count));
      const list = el('ul', 'rc-kb-list');
      if (!c.cards.length) list.append(el('li', 'rc-kb-empty', c.empty));
      for (const card of c.cards) list.append(cardEl(card));
      if (c.older) list.append(el('li', 'rc-kb-older', `and ${c.older} older`));
      lane.append(h, list);
      return lane;
    }));
    for (const lane of lanes.querySelectorAll('.rc-kb-lane')) {
      const top = v.scroll[lane.dataset.col];
      if (top) lane.querySelector('.rc-kb-list').scrollTop = top;
    }
    $('kb-ticket').hidden = !v.ticket;
    lanes.hidden = !!v.ticket;
    $('kb-pick').hidden = !!v.ticket;
    $('kb-pick').querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
  }
  function pickCol(id) {
    const v = view();
    if (!v.board || v.col === id) return;
    v.col = id;
    renderBoard();
  }
  function cardEl(c) {
    const li = el('li');
    const b = el('button', `rc-kb-card${c.priority === 'p1' ? ' rc-kb-p1' : ''}`);
    b.type = 'button';
    b.dataset.key = c.key;
    const top = el('span', 'rc-kb-top');
    top.append(el('span', 'rc-kb-num', c.num));
    if (c.priority !== 'none') top.append(el('span', `rc-kb-prio rc-kb-${c.priority}`, c.priority.toUpperCase()));
    if (c.age) top.append(el('span', 'rc-kb-age', c.age));
    b.append(top, el('span', 'rc-kb-title', c.title || c.num));
    if (c.marks.length) {
      const m = el('span', 'rc-kb-marks');
      for (const x of c.marks) m.append(el('span', `rc-kb-mark rc-tone-${x.tone}`, x.label));
      b.append(m);
    }
    if (c.thread) {
      const t = el('span', `rc-kb-thread rc-tone-${c.thread_tone}`);
      t.append(el('span', 'rc-kb-dot'), el('span', 'rc-kb-tname', c.thread));
      if (c.thread_state) t.append(el('span', 'rc-kb-tstate', RC_KB_THREAD[c.thread_state] || c.thread_state));
      b.append(t);
    }
    b.addEventListener('click', () => openTicket(c));
    li.append(b);
    return li;
  }
  // Swipe left or right on a phone: the next or the previous column.
  let swipe = null;
  $('kb-lanes').addEventListener('touchstart', e => {
    if (wide.matches || e.touches.length !== 1) { swipe = null; return; }
    swipe = { x: e.touches[0].clientX, y: e.touches[0].clientY };
  }, { passive: true });
  $('kb-lanes').addEventListener('touchend', e => {
    if (!swipe || !e.changedTouches.length) return;
    const dx = e.changedTouches[0].clientX - swipe.x;
    const dy = e.changedTouches[0].clientY - swipe.y;
    swipe = null;
    if (Math.abs(dx) < 60 || Math.abs(dx) < Math.abs(dy) * 1.5) return;
    const v = view();
    const ids = v.board?.columns.map(c => c.id) || [];
    const i = ids.indexOf(v.col) + (dx < 0 ? 1 : -1);
    if (i >= 0 && i < ids.length) pickCol(ids[i]);
  }, { passive: true });

  // ── A ticket: what the card holds, and its detail when it has a number ──
  async function openTicket(card, again) {
    const v = view();
    const sandbox = st.current;
    v.ticket = { card };
    const box = $('kb-ticket');
    if (!again) {
      box.replaceChildren(ticketView(card, null));
      renderBoard();
      $('s-chat').scrollTop = 0;
    }
    // A thread's own card has no ticket: the card is all there is.
    if (!card.n) return;
    const res = await api.get(rcKbPaths(sandbox, card.n));
    if (st.current !== sandbox || v.ticket?.card.key !== card.key) return;
    if (!res.ok) {
      if (res.status === 404) { box.replaceChildren(ticketView(card, null, 'This ticket is not on the board any more.')); return; }
      if (!again) box.replaceChildren(ticketView(card, null, rcFailText('Ticket', res).replace('Ticket not sent', 'The ticket could not be read')));
      return;
    }
    const t = rcKbTicket(res.data);
    const top = box.scrollTop;
    box.replaceChildren(ticketView(t?.card || card, t));
    box.scrollTop = top;
  }
  function closeTicket() {
    const v = view();
    v.ticket = null;
    renderBoard();
  }
  function ticketView(card, t, note) {
    const frag = document.createDocumentFragment();
    const back = el('button', 'rc-kb-back');
    back.type = 'button';
    back.append(icon('M14.5 5.5 8 12l6.5 6.5'), el('span', null, 'Board'));
    back.addEventListener('click', closeTicket);
    frag.append(back);
    const head_ = el('div', 'rc-kb-thead');
    head_.append(el('span', 'rc-kb-num', t?.num || card.num));
    const prio = t?.priority || card.priority;
    if (prio !== 'none') head_.append(el('span', `rc-kb-prio rc-kb-${prio}`, RC_KB_PRIORITY[prio]));
    const col = view().board?.columns.find(c => c.id === (t?.column || ''))?.title;
    if (col) head_.append(el('span', 'rc-kb-tcol', col));
    frag.append(head_, el('h2', 'rc-kb-ttitle', t?.title || card.title));
    if (card.marks.length) {
      const m = el('div', 'rc-kb-marks');
      for (const x of card.marks) m.append(el('span', `rc-kb-mark rc-tone-${x.tone}`, x.label));
      frag.append(m);
    }
    if (card.thread) {
      const th = el('p', `rc-kb-thread rc-tone-${card.thread_tone}`);
      th.append(el('span', 'rc-kb-dot'), el('span', 'rc-kb-tname', card.thread));
      if (card.thread_state) th.append(el('span', 'rc-kb-tstate', RC_KB_THREAD[card.thread_state] || card.thread_state));
      frag.append(th);
    }
    if (note) frag.append(el('p', 'rc-item-note', note));
    if (!card.n) frag.append(el('p', 'rc-muted rc-small-text', 'A thread of the plan, with no ticket of its own.'));
    if (t) {
      if (t.description) frag.append(el('p', 'rc-kb-desc', t.description));
      if (t.files.length) {
        const box = el('div', 'rc-kv');
        box.append(el('p', 'rc-kv-k', t.files.length === 1 ? '1 file' : `${t.files.length} files`));
        const ul = el('ul', 'rc-kb-files');
        for (const f of t.files) {
          const li = el('li');
          li.append(el('span', 'rc-file-path', f.name), el('span', 'rc-file-n', chatSize(f.size)));
          ul.append(li);
        }
        box.append(ul, el('p', 'rc-muted rc-small-text', 'Files open on your Mac.'));
        frag.append(box);
      }
      if (t.history.length) {
        const box = el('div', 'rc-kv');
        box.append(el('p', 'rc-kv-k', 'History'));
        const ul = el('ul', 'rc-kb-hist');
        for (const h of t.history) {
          const li = el('li', `rc-tone-${h.tone}`);
          li.append(el('span', 'rc-kb-when', h.when), el('span', null, h.text));
          ul.append(li);
        }
        box.append(ul);
        frag.append(box);
      }
    } else if (card.n && !note) {
      frag.append(el('p', 'rc-muted', 'Loading…'));
    }
    return frag;
  }

  // ══ Level 2: approving with Face ID ═══════════════════════════════════
  // Lists and details are read with the cookie; every action goes through
  // the sheet, a challenge bound to what is shown (its digest), Face ID, and
  // the assertion, in that order. The challenge, the options and the
  // assertion live in the sheet's locals for the one request, and are
  // dropped when the sheet closes or the page hides.
  const l2 = rcL2Client(api);
  const LISTS = [
    { what: 'plans', key: 'plans', title: 'Plans to approve', count: 'plans' },
    { what: 'merges', key: 'merges', title: 'Merges proposed', count: 'merges' },
    { what: 'network', key: 'requests', title: 'Network requests', count: 'network' },
    { what: 'stopped', key: 'sandboxes', title: 'Stopped sandboxes', count: 'stopped' },
  ];
  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined && text !== null) e.textContent = String(text);
    return e;
  };
  const str = v => (typeof v === 'string' ? v : v === null || v === undefined ? '' : String(v));

  async function refreshL2() {
    const res = await l2.approvals();
    if (res.ok && res.data && typeof res.data === 'object') st.l2 = res.data;
    else if (res.status === 404) st.l2 = { level2: false };
    renderBadge();
    return res;
  }
  function renderBadge() {
    const c = st.l2?.level2 && st.l2.counts && typeof st.l2.counts === 'object' ? st.l2.counts : null;
    const n = c ? ['plans', 'merges', 'network', 'stopped'].reduce((t, k) => t + (Math.max(0, Math.floor(Number(c[k]))) || 0), 0) : 0;
    const b = $('badge');
    b.textContent = n > 99 ? '99+' : String(n);
    b.hidden = !n;
    $('approvals-btn').setAttribute('aria-label', n ? `Approvals, ${n} waiting` : 'Approvals');
  }
  $('approvals-btn').addEventListener('click', () => {
    st.l2From = st.screen === 's-chat' ? st.current : null;
    if (st.screen === 's-chat') { saveDraft(); stopRecording(false); disarmInterrupt(); }
    showApprovals();
  });
  function leaveApprovals() {
    const back = st.l2From;
    st.l2From = null;
    st.item = null;
    if (back && st.current === back) {
      show('s-chat');
      renderChatTitle();
      renderTail();
      setView(view().mode);
      return;
    }
    showLeaders();
  }

  // ── The lists ──
  async function showApprovals(done) {
    st.item = null;
    stopPasskeyPoll();
    setTitle('Approvals', '');
    show('s-approvals');
    $('ap-done').textContent = done || '';
    $('ap-done').hidden = !done;
    const res = await refreshL2();
    if (st.screen !== 's-approvals') return;
    const a = st.l2;
    const block = res.ok ? rcApprovalsBlock(a) : rcL2Fail(res);
    $('ap-block').hidden = !block;
    $('ap-block-text').textContent = block;
    // Face ID is enabled from here once the Mac has level 2 on.
    $('ap-enable').hidden = !(a?.level2 === true && (a.passkey !== true || a.rp_ok === false));
    if (!a?.level2) { $('ap-lists').replaceChildren(); $('ap-empty').hidden = true; return; }
    const got = await Promise.all(LISTS.map(L => l2.list(L.what)));
    if (st.screen !== 's-approvals') return;
    const sections = [];
    LISTS.forEach((L, i) => {
      const r = got[i];
      const items = r.ok && r.data && Array.isArray(r.data[L.key]) ? r.data[L.key] : [];
      // A list the Mac could not read says so, quietly, in its place.
      if (!r.ok && r.status !== 403) {
        const sec = el('section', 'rc-ap-sec');
        sec.append(el('h2', 'rc-ap-h', L.title), el('p', 'rc-muted rc-small-text rc-ap-fail', rcL2Fail(r)));
        sections.push(sec);
        return;
      }
      if (!items.length) return;
      const sec = el('section', 'rc-ap-sec');
      const h = el('h2', 'rc-ap-h', L.title);
      const n = el('span', 'rc-ap-n', items.length);
      h.append(' ', n);
      const ul = el('ul', 'rc-ap-list');
      for (const it of items) ul.append(listRow(L.what, it));
      sec.append(h, ul);
      sections.push(sec);
    });
    $('ap-lists').replaceChildren(...sections);
    $('ap-empty').hidden = sections.length > 0;
  }
  function listRow(what, it) {
    const li = el('li');
    const b = el('button', `rc-ap-row rc-ap-${what}`);
    b.type = 'button';
    let id, main, meta;
    if (what === 'plans') { id = str(it.id); main = str(it.summary) || id; meta = `${str(it.sandbox)} · ${Number(it.threads) || 0} threads`; }
    else if (what === 'merges') { id = str(it.id); main = `Merge of ${str(it.sandbox)}`; meta = `${str(it.variant)} · ${Number(it.branches) || 0} branches`; }
    else if (what === 'network') { id = str(it.id); main = str(it.host); meta = str(it.sandbox); }
    else { id = str(it.name); main = str(it.name); meta = [str(it.agent) || (it.chat ? 'chat' : ''), str(it.workspace)].filter(Boolean).join(' · '); }
    b.append(el('span', 'rc-ap-main', main), el('span', 'rc-ap-meta', meta));
    b.addEventListener('click', () => openItem(what, id));
    li.append(b);
    return li;
  }

  // ── A detail ──
  async function openItem(what, id, said) {
    closeSheet();
    st.item = { what, id, data: null, digest: '' };
    const titles = { plans: 'Plan', merges: 'Merge', network: 'Network request', stopped: 'Stopped sandbox' };
    setTitle(titles[what], '');
    show('s-item');
    itemStatus(said || '');
    $('item').replaceChildren(el('p', 'rc-muted', 'Loading…'));
    $('item-acts').replaceChildren();
    const res = await l2.detail(what, id);
    if (st.item?.what !== what || st.item.id !== id) return;
    if (!res.ok || !res.data) {
      $('item').replaceChildren(el('p', 'rc-muted', res.status === 404 ? 'This is not waiting any more.' : rcL2Fail(res)));
      return;
    }
    const d = res.data;
    const key = { plans: 'plan', merges: 'merge', network: 'request', stopped: 'sandbox' }[what];
    const data = d[key] && typeof d[key] === 'object' ? d[key] : {};
    st.item.data = data;
    st.item.digest = typeof d.digest === 'string' && /^[0-9a-f]{64}$/.test(d.digest) ? d.digest : '';
    const body = what === 'plans' ? planView(data) : what === 'merges' ? mergeView(data) : what === 'network' ? networkView(data) : stoppedView(data, d.confirm);
    $('item').replaceChildren(...body);
    renderActs(what, data);
  }
  function itemStatus(text, tone) {
    const p = $('item-status');
    p.textContent = text;
    p.className = `rc-item-status${tone ? ` rc-t-${tone}` : ''}`;
    p.hidden = !text;
  }
  // A field: a quiet label and its value, as text. Long text folds.
  function field_(label, value, opts = {}) {
    const box = el('div', `rc-kv${opts.mono ? ' rc-kv-mono' : ''}`);
    box.append(el('p', 'rc-kv-k', label));
    const v = el('p', 'rc-kv-v', value);
    box.append(v);
    if (opts.fold && (value.length > 220 || value.split('\n').length > 4)) {
      v.classList.add('rc-folded');
      const t = el('button', 'rc-fold', 'Show all');
      t.type = 'button';
      t.setAttribute('aria-expanded', 'false');
      t.addEventListener('click', () => {
        const open = v.classList.toggle('rc-folded') === false;
        t.textContent = open ? 'Fold' : 'Show all';
        t.setAttribute('aria-expanded', String(open));
      });
      box.append(t);
    }
    return box;
  }
  function head(title, sub) {
    const h = el('div', 'rc-item-head');
    h.append(el('h2', 'rc-item-title', title));
    if (sub) h.append(el('p', 'rc-item-sub', sub));
    return h;
  }
  function planView(p) {
    const out = [head(str(p.summary) || 'A plan', `${str(p.sandbox)} · ${str(p.variant) === 'shared' ? 'one shared branch' : 'a branch per thread'}`)];
    if (str(p.justification)) out.push(field_('Why this way', str(p.justification), { fold: true }));
    const dirty = Number(p.main_uncommitted) || 0;
    if (dirty) out.push(el('p', 'rc-item-warn', `${dirty} uncommitted ${dirty === 1 ? 'change' : 'changes'} in the main checkout.`));
    const threads = Array.isArray(p.threads) ? p.threads : [];
    out.push(el('h3', 'rc-item-h', threads.length === 1 ? '1 thread' : `${threads.length} threads`));
    for (const t of threads) {
      const card = el('div', 'rc-thread');
      const top = el('div', 'rc-thread-top');
      top.append(el('span', 'rc-thread-name', str(t.name)));
      const meta = [str(t.model), str(t.repo) && str(t.repo) !== '.' ? str(t.repo) : '', str(t.ticket)].filter(Boolean).join(' · ');
      if (meta) top.append(el('span', 'rc-thread-meta', meta));
      card.append(top);
      if (str(t.scope)) card.append(field_('Scope', str(t.scope), { fold: true }));
      if (str(t.goal)) card.append(field_('Goal', str(t.goal), { fold: true }));
      out.push(card);
    }
    return out;
  }
  const short = h => str(h).slice(0, 7);
  function mergeView(m) {
    const out = [head(`Merge of ${str(m.sandbox)}`, str(m.variant) === 'shared' ? 'one shared branch' : 'a branch per thread')];
    const order = Array.isArray(m.order) ? m.order.map(str) : [];
    if (order.length) {
      const ol = el('ol', 'rc-order');
      for (const o of order) ol.append(el('li', null, o));
      const box = el('div', 'rc-kv');
      box.append(el('p', 'rc-kv-k', 'In this order'), ol);
      out.push(box);
    }
    if (str(m.note)) out.push(field_('The lead\'s note', str(m.note), { fold: true }));
    const dist = Array.isArray(m.distillate) ? m.distillate : [];
    if (dist.length) {
      const ul = el('ul', 'rc-dist');
      for (const d of dist) {
        const li = el('li', `rc-dist-${d?.kind === 'warning' ? 'warning' : 'decision'}`);
        li.append(el('span', 'rc-dist-k', d?.kind === 'warning' ? 'Warning' : 'Decision'), el('span', null, str(d?.text)));
        ul.append(li);
      }
      const box = el('div', 'rc-kv');
      box.append(el('p', 'rc-kv-k', 'Kept for later sessions'), ul);
      out.push(box);
    }
    for (const b of Array.isArray(m.branches) ? m.branches : []) {
      const card = el('div', 'rc-branch');
      const top = el('div', 'rc-thread-top');
      top.append(el('span', 'rc-thread-name', str(b.branch) || str(b.thread)));
      top.append(el('span', 'rc-thread-meta', b.tip ? short(b.tip) : 'unreadable'));
      card.append(top);
      const base = b.base && typeof b.base === 'object' ? b.base : {};
      const ins = Number(b.insertions) || 0, del = Number(b.deletions) || 0;
      card.append(el('p', 'rc-branch-line', `into ${str(base.branch)} at ${base.tip ? short(base.tip) : 'unreadable'}${str(b.repo) && str(b.repo) !== '.' ? ` (${str(b.repo)})` : ''}`));
      const stat = el('p', 'rc-branch-stat');
      stat.append(el('span', null, `${Number(b.ahead) || 0} ${Number(b.ahead) === 1 ? 'commit' : 'commits'} · ${Number(b.files_changed) || 0} files `), el('span', 'rc-ins', `+${ins}`), ' ', el('span', 'rc-del', `−${del}`));
      card.append(stat);
      if (b.known_conflict === true) card.append(el('p', 'rc-item-warn', 'An earlier attempt stopped at a conflict on this branch.'));
      const files = Array.isArray(b.files) ? b.files : [];
      if (files.length) {
        const ul = el('ul', 'rc-files');
        for (const f of files) {
          const li = el('li');
          li.append(el('span', 'rc-file-path', str(f?.path)));
          const n = el('span', 'rc-file-n');
          if (f?.insertions === null || f?.deletions === null) n.textContent = 'binary';
          else n.append(el('span', 'rc-ins', `+${Number(f?.insertions) || 0}`), ' ', el('span', 'rc-del', `−${Number(f?.deletions) || 0}`));
          li.append(n);
          ul.append(li);
        }
        const fold = el('details', 'rc-files-box');
        const sum = el('summary', null, files.length === 1 ? '1 file' : `${files.length} files`);
        fold.append(sum, ul);
        if (b.files_truncated === true) fold.append(el('p', 'rc-muted rc-small-text', 'More files changed than are listed here: see them on your Mac.'));
        card.append(fold);
      }
      out.push(card);
    }
    return out;
  }
  function networkView(r) {
    const out = [head(str(r.host), `asked by ${str(r.sandbox)}`)];
    if (str(r.detail)) out.push(field_('What sbx says', str(r.detail), { fold: true, mono: true }));
    if (str(r.created_at)) out.push(field_('Asked at', str(r.created_at)));
    const opts = Array.isArray(r.options) ? r.options : [];
    const macOnly = Array.isArray(r.mac_only) ? r.mac_only.map(str).filter(Boolean) : [];
    const allowHere = opts.some(o => /allow/i.test(`${str(o?.id)} ${str(o?.label)}`));
    if (!opts.length) out.push(el('p', 'rc-item-note', 'Answer this on your Mac.'));
    else if (!allowHere) out.push(el('p', 'rc-item-note', 'Allowing this needs your Mac.'));
    if (macOnly.length) out.push(el('p', 'rc-muted rc-small-text', `On your Mac only: ${macOnly.join(', ')}.`));
    return out;
  }
  function stoppedView(sb, confirm) {
    const out = [head(str(sb.name), [str(sb.agent) || (sb.chat ? 'chat' : ''), str(sb.created_at)].filter(Boolean).join(' · '))];
    const lines = Array.isArray(confirm) ? confirm.map(str).filter(Boolean) : [];
    const box = el('div', 'rc-confirm-lines');
    // The daemon's own sentences, verbatim, as text.
    for (const l of lines) box.append(el('p', null, l));
    out.push(box);
    return out;
  }
  // The decisions of an item: each opens the sheet; nothing acts on a tap.
  const DANGER = /\b(deny|dismiss|refuse|block)\b/i;
  function renderActs(what, data) {
    const acts = $('item-acts');
    const can = st.l2?.level2 === true && st.l2.passkey === true && st.l2.rp_ok !== false && !!st.item?.digest;
    if (!can) {
      acts.replaceChildren(el('p', 'rc-item-note', rcApprovalsBlock(st.l2) || 'This cannot be approved from the phone.'));
      return;
    }
    const btn = (label, kind, cls, option) => {
      const b = el('button', `rc-btn ${cls}`, label);
      b.type = 'button';
      b.dataset.kind = kind;
      b.addEventListener('click', () => openSheet(kind, option));
      return b;
    };
    const list = [];
    if (what === 'plans') list.push(btn('Refuse', 'plan_refuse', 'rc-danger'), btn('Approve', 'plan_approve', 'rc-primary'));
    else if (what === 'merges') list.push(btn('Merge', 'merge', 'rc-primary'));
    else if (what === 'stopped') list.push(btn('Remove', 'sandbox_remove', 'rc-danger'));
    else for (const o of Array.isArray(data.options) ? data.options : []) {
      if (!o || typeof o.id !== 'string') continue;
      const label = str(o.label) || o.id;
      list.push(btn(label.charAt(0).toUpperCase() + label.slice(1), 'network_respond', DANGER.test(`${o.id} ${label}`) ? 'rc-danger' : 'rc-primary', { id: o.id, label }));
    }
    acts.replaceChildren(...list);
  }

  // ── The sheet: what will happen, to what; then Face ID ──
  let sheet = null; // { kind, option, ctl, token } while open
  let sheetSeq = 0;
  const GO = { plan_approve: 'Approve with Face ID', plan_refuse: 'Refuse with Face ID', merge: 'Merge with Face ID', network_respond: 'Answer with Face ID', sandbox_remove: 'Remove with Face ID' };
  const SHEET_TITLE = { plan_approve: 'Approve this plan?', plan_refuse: 'Refuse this plan?', merge: 'Run this merge?', network_respond: 'Answer this request?', sandbox_remove: 'Remove this sandbox?' };
  function openSheet(kind, option) {
    if (!st.item?.data || !RC_KINDS.includes(kind)) return;
    closeSheet();
    sheet = { kind, option: option || null, ctl: null, token: ++sheetSeq };
    $('sheet-title').textContent = kind === 'network_respond' ? `Answer "${option?.label ?? ''}"?` : SHEET_TITLE[kind];
    $('sheet-line').textContent = rcActLine(kind, st.item.data, option);
    $('sheet-status').textContent = '';
    const go = $('sheet-go');
    go.textContent = GO[kind];
    const danger = kind === 'plan_refuse' || kind === 'sandbox_remove' || (kind === 'network_respond' && DANGER.test(`${option?.id} ${option?.label}`));
    go.className = `rc-btn ${danger ? 'rc-danger-solid' : 'rc-primary'}`;
    go.disabled = false;
    $('sheet').dataset.sheet = kind;
    $('sheet').hidden = false;
    $('veil').hidden = false;
    go.focus({ preventScroll: true });
  }
  // Closing drops everything of the ceremony: an outstanding Face ID prompt
  // is aborted, a late answer is ignored.
  function closeSheet() {
    if (!sheet) return;
    sheet.ctl?.abort();
    sheet = null;
    $('sheet').hidden = true;
    $('veil').hidden = true;
  }
  $('sheet-cancel').addEventListener('click', closeSheet);
  $('veil').addEventListener('click', closeSheet);
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'hidden') closeSheet(); });
  window.addEventListener('pagehide', closeSheet);

  $('sheet-go').addEventListener('click', async () => {
    const s = sheet;
    const item = st.item;
    if (!s || !item?.digest || $('sheet-go').disabled) return;
    const live = () => sheet === s && st.item === item;
    const say = (text, tone) => { if (live()) { $('sheet-status').textContent = text; $('sheet-status').className = `rc-sheet-status${tone ? ` rc-t-${tone}` : ''}`; } };
    const target = item.what === 'stopped' ? str(item.data.name) : item.id;
    $('sheet-go').disabled = true;
    say('Asking the Mac…', 'busy');
    const ch = await l2.challenge(s.kind, target, item.digest, s.option?.id);
    if (!live()) return;
    if (!ch.ok) return failed(ch, s);
    const publicKey = rcGetOptions(ch.data?.public_key);
    if (!publicKey) { $('sheet-go').disabled = false; return say('The Mac sent a request this page cannot use. Nothing was done.', 'err'); }
    say('Waiting for Face ID…', 'busy');
    s.ctl = new AbortController();
    let cred;
    try {
      cred = await navigator.credentials.get({ publicKey, signal: s.ctl.signal });
    } catch (e) {
      if (!live()) return;
      $('sheet-go').disabled = false;
      const why = rcCeremonyError(e);
      return say(why === 'cancelled' ? 'Face ID was cancelled. Nothing was done.' : why === 'no_authenticator' ? 'Face ID is not available here. Nothing was done.' : 'Face ID did not finish. Nothing was done.', 'err');
    }
    if (!live()) return;
    const assertion = rcAssertionBody(cred);
    if (!assertion) { $('sheet-go').disabled = false; return say('Face ID gave an answer this page cannot read. Nothing was done.', 'err'); }
    say('Sending…', 'busy');
    const res = await l2.act(s.kind, target, assertion, s.option?.id);
    if (!live()) return;
    if (!res.ok) return failed(res, s);
    closeSheet();
    showApprovals(rcActDone(s.kind, res.data));
  });
  function failed(res, s) {
    if (res.data?.reason === 'changed') {
      // Read it again: the detail is fetched anew, the sheet closes.
      const it = st.item;
      closeSheet();
      if (it) openItem(it.what, it.id, RC_L2_REASON.changed);
      return;
    }
    if (sheet !== s) return;
    $('sheet-status').textContent = rcL2Fail(res);
    $('sheet-status').className = 'rc-sheet-status rc-t-err';
    $('sheet-go').disabled = false;
    if (res.data?.reason === 'no_passkey' || res.data?.reason === 'level2_off') { $('sheet-go').disabled = true; refreshL2(); }
  }

  // ── Enabling Face ID on this device ──
  let pkPoll = null;
  function stopPasskeyPoll() { if (pkPoll) { clearTimeout(pkPoll); pkPoll = null; } }
  function pkShow(state, check) {
    $('pk-check').hidden = !check;
    if (check) $('pk-check').textContent = /^\d{6}$/.test(check) ? `${check.slice(0, 3)} ${check.slice(3)}` : check;
    $('pk-title').textContent = state === 'pending' ? 'Type this number on your Mac' : state === 'registered' ? 'Face ID is ready' : 'Enable Face ID';
    $('pk-text').textContent = RC_PASSKEY_TEXT[state] || RC_PASSKEY_TEXT.failed;
    $('pk-go').hidden = state !== 'granted';
    $('pk-go').disabled = false;
    $('pk-again').hidden = !['refused', 'expired', 'cancelled', 'failed', 'rejected', 'no_authenticator', 'exists'].includes(state);
    $('s-passkey').dataset.state = state;
  }
  async function startPasskey() {
    stopPasskeyPoll();
    setTitle('Face ID', '');
    show('s-passkey');
    pkShow('starting');
    $('pk-text').textContent = 'Asking the Mac…';
    const res = await l2.passkeyStart();
    if (st.screen !== 's-passkey') return;
    if (!res.ok || typeof res.data?.request_id !== 'string') {
      if (res.data?.reason === 'level2_off') return pkShow('off');
      pkShow('failed');
      $('pk-text').textContent = rcL2Fail(res);
      return;
    }
    pkShow('pending', String(res.data.check || ''));
    const until = Date.now() + 6 * 60 * 1000;
    const poll = async () => {
      pkPoll = null;
      if (st.screen !== 's-passkey') return;
      if (Date.now() > until) return pkShow('expired');
      const r = await l2.passkeyStatus();
      if (st.screen !== 's-passkey') return;
      const state = r.ok ? r.data?.state : null;
      if (state === 'granted') return pkShow('granted');
      if (state === 'refused') return pkShow('refused');
      if (state === 'registered') return pkShow('registered');
      if (state === 'none') return pkShow('expired');
      pkPoll = setTimeout(poll, 2000);
    };
    pkPoll = setTimeout(poll, 1500);
  }
  $('ap-enable').addEventListener('click', startPasskey);
  $('pk-again').addEventListener('click', startPasskey);
  // Safari asks for Face ID only from a tap: the key is created on this one.
  $('pk-go').addEventListener('click', async () => {
    const go = $('pk-go');
    if (go.disabled) return;
    go.disabled = true;
    if (!window.PublicKeyCredential || !navigator.credentials?.create) return pkShow('no_authenticator');
    try {
      if (typeof PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable === 'function'
        && !(await PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable())) return pkShow('no_authenticator');
    } catch (_) { /* unknown: let create() say */ }
    const o = await l2.passkeyOptions();
    if (st.screen !== 's-passkey') return;
    if (!o.ok) { pkShow(o.data?.reason === 'no_grant' ? 'expired' : 'failed'); if (o.data?.reason) $('pk-text').textContent = rcL2Fail(o); return; }
    const publicKey = rcCreateOptions(o.data?.public_key);
    if (!publicKey) { pkShow('failed'); $('pk-text').textContent = 'The Mac sent options this page cannot use. Nothing was saved.'; return; }
    let cred;
    try {
      cred = await navigator.credentials.create({ publicKey });
    } catch (e) {
      if (st.screen === 's-passkey') pkShow(rcCeremonyError(e));
      return;
    }
    if (st.screen !== 's-passkey') return;
    const body = rcRegistrationBody(cred);
    if (!body) { pkShow('failed'); $('pk-text').textContent = 'The browser gave an answer this page cannot read. Nothing was saved.'; return; }
    const r = await l2.passkeyRegister(body);
    if (st.screen !== 's-passkey') return;
    // no_grant: the grant expired or was used (v1.0); passkey: a check failed.
    if (!r.ok) return pkShow(r.data?.reason === 'passkey' ? 'rejected' : r.data?.reason === 'no_grant' ? 'expired' : 'failed');
    pkShow('registered');
    refreshL2();
  });

  // ── Navigation ──
  $('back').addEventListener('click', () => {
    if (st.screen === 's-item' || st.screen === 's-passkey') return showApprovals();
    if (st.screen === 's-approvals') return leaveApprovals();
    if (history.state && history.state.leader) history.back();
    else showLeaders();
  });
  window.addEventListener('popstate', () => { if (st.current && st.screen === 's-chat') showLeaders(); });

  // ── The keyboard must not hide the composer ──
  // The body is fixed to the visual viewport; with the keyboard up, the
  // bottom safe area is the keyboard's, not the home indicator's.
  const vv = window.visualViewport;
  let tallest = 0;
  function fit() {
    if (!vv) return;
    tallest = Math.max(tallest, vv.height);
    const root = document.documentElement;
    root.style.setProperty('--vvh', `${vv.height}px`);
    root.style.setProperty('--vvt', `${vv.offsetTop}px`);
    root.classList.toggle('rc-kb', vv.height < tallest - 120);
  }
  if (vv) {
    vv.addEventListener('resize', fit);
    vv.addEventListener('scroll', fit);
    window.addEventListener('orientationchange', () => { tallest = 0; setTimeout(fit, 300); });
    fit();
  }
  field.addEventListener('focus', () => setTimeout(() => { fit(); if ($('log').scrollHeight - $('log').scrollTop - $('log').clientHeight < 200) scrollToEnd(); }, 300));

  boot();
})();
