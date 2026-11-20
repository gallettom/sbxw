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
  const prefs = {
    name() {
      try {
        const v = JSON.parse(localStorage.getItem(PREFS) || '{}');
        return v && typeof v.device_name === 'string' ? rcCleanName(v.device_name) : '';
      } catch (_) { return ''; }
    },
    setName(name) {
      try { localStorage.setItem(PREFS, JSON.stringify({ device_name: rcCleanName(name) })); } catch (_) {}
    },
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
  };

  const banner = $('unreachable');
  const api = rcClient((path, init) => fetch(path, init), {
    reach: ok => { banner.hidden = ok || !st.paired && !st.pairing; },
    unpaired: () => unpaired('This device is no longer paired', 'Pair it again from your Mac: open the pairing link or scan the QR code that sbxw shows.'),
  });

  // ── Screens ──
  const SCREENS = ['s-loading', 's-pair', 's-unpaired', 's-leaders', 's-chat'];
  function show(id) {
    for (const s of SCREENS) $(s).hidden = s !== id;
    $('back').hidden = id !== 's-chat';
    $('mark').hidden = id === 's-chat';
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
    if (st.current) {
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
  mic.addEventListener('click', async () => {
    if (micWhy) { st.hint = micWhy; renderStatus(); return; }
    if (rec.state === 'transcribing' || rec.starting) return;
    if (rec.recorder) { rec.recorder.stop(); return; }
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
    if (st.note?.tone !== 'warn') st.note = null;
    startWave(stream);
    setRec('recording');
    const tick = () => {
      const s = Math.floor((Date.now() - rec.started) / 1000);
      $('rec-time').textContent = rcClock(s);
      if (s >= RC_VOICE_MAX_S && rec.recorder) rec.recorder.stop();
    };
    tick();
    rec.timer = setInterval(tick, 250);
  });

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

  // ── Navigation ──
  $('back').addEventListener('click', () => {
    if (history.state && history.state.leader) history.back();
    else showLeaders();
  });
  window.addEventListener('popstate', () => { if (st.current) showLeaders(); });

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
