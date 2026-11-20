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
    sending: false,
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
  function renderChatTitle() {
    const l = leader();
    const sub = !l ? 'not running' : l.asks ? 'needs you' : rcStateLabel(l.state);
    setTitle(l?.name || st.current, sub);
    $('interrupt').dataset.name = l?.name || st.current;
    if (!$('interrupt').classList.contains('rc-armed')) $('interrupt').textContent = 'Interrupt';
  }

  async function openLeader(sandbox) {
    st.current = sandbox;
    st.items = [];
    st.nextBefore = null;
    st.latestSeq = 0;
    $('field').value = st.drafts.get(sandbox) || '';
    note('');
    $('mic-note').textContent = '';
    disarmInterrupt();
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
    $('rows').replaceChildren(...rows.map(rowEl));
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
      d.textContent = r.count === 1 ? '1 step' : `${r.count} steps`;
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
  function note(text, ok) {
    const n = $('composer-note');
    n.textContent = text;
    n.classList.toggle('rc-ok', !!ok);
    n.hidden = !text;
  }
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
  $('send').addEventListener('click', async () => {
    const sandbox = st.current;
    const raw = field.value;
    const text = raw.trim();
    if (!sandbox || !text || st.sending) return;
    st.sending = true;
    $('send').disabled = true;
    note('');
    const res = await api.message(sandbox, text);
    st.sending = false;
    $('send').disabled = false;
    if (!res.ok) { note(rcFailText('Message', res)); return; }
    // Sent: what was sent leaves the field; anything typed meanwhile stays.
    if (st.current === sandbox) {
      if (field.value === raw) field.value = '';
      else if (field.value.startsWith(raw)) field.value = field.value.slice(raw.length).replace(/^\s+/, '');
      field.dispatchEvent(new Event('input'));
      note('Sent.', true);
      setTimeout(() => { if ($('composer-note').textContent === 'Sent.') note(''); }, 2500);
    } else {
      st.drafts.delete(sandbox);
    }
  });

  // ── Interrupt: two taps ──
  let armTimer = null;
  function disarmInterrupt() {
    clearTimeout(armTimer);
    const b = $('interrupt');
    b.classList.remove('rc-armed');
    b.textContent = 'Interrupt';
    $('interrupt-cancel').hidden = true;
  }
  $('interrupt').addEventListener('click', async () => {
    const b = $('interrupt');
    const sandbox = st.current;
    if (!sandbox) return;
    if (!b.classList.contains('rc-armed')) {
      b.classList.add('rc-armed');
      b.textContent = `Interrupt ${b.dataset.name || sandbox}?`;
      $('interrupt-cancel').hidden = false;
      armTimer = setTimeout(disarmInterrupt, 6000);
      return;
    }
    disarmInterrupt();
    b.disabled = true;
    const res = await api.interrupt(sandbox);
    b.disabled = false;
    note(res.ok ? 'Interrupt sent.' : rcFailText('Interrupt', res), res.ok);
  });
  $('interrupt-cancel').addEventListener('click', disarmInterrupt);

  // ── Dictation: into the field, never sent ──
  const mic = $('mic');
  const micNote = $('mic-note');
  const rec = { stream: null, recorder: null, chunks: [], timer: null, started: 0, busy: false };
  const micWhy = !window.isSecureContext
    ? 'Dictation needs the HTTPS address of sbxw.'
    : !(navigator.mediaDevices && navigator.mediaDevices.getUserMedia) || typeof MediaRecorder === 'undefined'
      ? 'This browser cannot record audio.'
      : '';
  if (micWhy) {
    mic.classList.add('rc-off');
    mic.setAttribute('aria-label', `Dictate (unavailable: ${micWhy})`);
  }
  mic.addEventListener('click', async () => {
    if (micWhy) { micNote.textContent = micWhy; return; }
    if (rec.busy) return;
    if (rec.recorder) { rec.recorder.stop(); return; }
    let stream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
    } catch (e) {
      micNote.textContent = e && e.name === 'NotAllowedError'
        ? 'Microphone refused. Allow it for this site in Settings, then try again.'
        : e && e.name === 'NotFoundError' ? 'No microphone found.' : 'The microphone could not start.';
      return;
    }
    const recorder = new MediaRecorder(stream);
    Object.assign(rec, { stream, recorder, chunks: [], started: Date.now() });
    recorder.addEventListener('dataavailable', e => { if (e.data && e.data.size) rec.chunks.push(e.data); });
    recorder.addEventListener('stop', () => finishRecording(recorder.mimeType));
    recorder.start();
    mic.classList.add('rc-recording');
    mic.setAttribute('aria-label', 'Stop dictation');
    const tick = () => {
      const s = Math.floor((Date.now() - rec.started) / 1000);
      micNote.textContent = `Recording ${s} s · tap the mic to stop`;
      if (s >= RC_VOICE_MAX_S) recorder.stop();
    };
    tick();
    rec.timer = setInterval(tick, 500);
  });

  async function finishRecording(type) {
    clearInterval(rec.timer);
    rec.stream?.getTracks().forEach(t => t.stop());
    const blob = new Blob(rec.chunks, { type: type || 'audio/mp4' });
    Object.assign(rec, { stream: null, recorder: null, chunks: [], busy: true });
    mic.classList.remove('rc-recording');
    mic.setAttribute('aria-label', 'Dictate');
    const sandbox = st.current;
    micNote.textContent = 'Transcribing…';
    let res;
    try {
      res = await api.transcribe(await toPcm(blob));
    } catch (_) {
      res = null;
    }
    rec.busy = false;
    if (!res) { micNote.textContent = 'This recording could not be read.'; return; }
    if (!res.ok) {
      micNote.textContent = res.status === 503 ? 'Dictation is not installed on the Mac.' : rcFailText('Recording', res).replace('not sent', 'not transcribed');
      return;
    }
    const words = typeof res.data?.text === 'string' ? res.data.text : '';
    if (!words.trim()) { micNote.textContent = 'No words heard.'; return; }
    if (st.current === sandbox) {
      field.value = rcWithDictation(field.value, words);
      field.dispatchEvent(new Event('input'));
      micNote.textContent = 'Added to the field: read it, then Send.';
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
