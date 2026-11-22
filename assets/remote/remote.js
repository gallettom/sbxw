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
    models: [],          // the model list from /r/models
    modelsAsked: false,
    skills: [],          // the skill names from /r/skills
    skillsAsked: false,
    ctlBusy: false,      // a model or lead change is in flight
    leadArm: null,       // which way a first tap on Lead armed
    current: null,       // the open leader's sandbox
    items: [],           // its history, oldest first
    nextBefore: null,
    latestSeq: 0,
    loadingOlder: false,
    newer: null,         // the in-flight ?after= read: { at } (its start)
    newerAgain: false,
    loaded: null,        // the leader whose first page is drawn
    hiddenAt: 0,         // when the page was last hidden (0: visible)
    drafts: new Map(),   // per leader: what was typed and not sent (memory only)
    trays: new Map(),    // per leader: the files picked for its next message
    kbTrays: new Map(),  // per leader: the files picked for a new ticket
    sending: false,
    note: null,          // the status line's last outcome: { text, tone }
    hint: '',            // a quieter line (dictation's results)
    shownSeq: 0,         // rows up to here are on screen: newer ones arrive
    es: null,
    probe: null,
    paired: false,
    screen: 's-loading',
    l2: null,            // GET /r/approvals: { level2, passkey, rp_ok, counts }
    apLists: [], apFilter: undefined, apTouched: false, apWanted: null, apSeq: 0, apSeen: '', apLoading: false, apBase: null, apIds: new Set(), apToastTimer: null, apToastTo: null,
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
    if (id !== 's-chat') { $('state').hidden = true; $('interrupt').hidden = true; $('ctl').hidden = true; $('skill-list').hidden = true; $('to-end').hidden = true; disarmInterrupt(); } else { loadModels(); loadSkills(); }
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
    if (push) push.dropBrowser();
    stopEvents();
    elsewhere(false);
    st.current = null;
    st.items = [];
    st.drafts.clear();
    for (const t of st.trays.values()) t.clear();
    st.trays.clear();
    for (const t of st.kbTrays.values()) t.clear();
    st.kbTrays.clear();
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
    refreshPush();
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
    // Each (re)opened stream catches up on what it may have missed while
    // it was down: one read, then the nudges take over.
    es.addEventListener('open', () => { banner.hidden = true; if (st.current) fetchNewer(); });
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
      // A newer view of this device took the stream's place (v1.2): say so
      // and wait for a tap; reconnecting by itself would take it back.
      if (d?.reason === 'too_many') { elsewhere(true); return; }
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
  function elsewhere(on) {
    st.elsewhere = on;
    $('elsewhere').hidden = !on;
  }
  $('elsewhere').addEventListener('click', async () => {
    if (!st.elsewhere) return;
    elsewhere(false);
    startEvents();
    const l = await api.leaders();
    if (l.ok) setLeaders(l.data);
    if (st.current) fetchNewer();
    refreshL2();
  });
  function stopEvents() {
    if (st.es) { st.es.close(); st.es = null; }
    if (st.probe) { clearTimeout(st.probe); st.probe = null; }
  }
  function scheduleProbe(ms) {
    if (st.probe || !st.paired || st.elsewhere) return;
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
  // Back from a locked screen or another app: the stream may have died
  // without a word (iOS suspends the page and its connection), so after
  // RC_AWAY_MS away it is opened again, and the conversation is read once
  // whatever the stream's state. No polling: the stream stays the trigger.
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState !== 'visible') { st.hiddenAt = Date.now(); return; }
    const away = st.hiddenAt ? Date.now() - st.hiddenAt : 0;
    st.hiddenAt = 0;
    if (!st.paired || st.elsewhere) return;
    if (!st.es) { scheduleProbe(10); return; }
    if (away >= RC_AWAY_MS || st.es.readyState === 2) startEvents();
    else if (st.current) fetchNewer();
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
    setTitle('Sessions', st.me?.name ? `on ${st.me.name}` : '');
    show('s-leaders');
    renderLeaders();
  }

  // Two groups (API.md 2.4, v1.5): the team leaders, then the running
  // sandboxes with no team; those that need the person first in each. Names
  // and the read-only reason are set as text.
  function renderLeaders() {
    const ul = $('leaders');
    const now = Date.now();
    const groups = rcGroups(st.leaders);
    const rows = [];
    for (const g of groups) {
      const head = document.createElement('li');
      head.className = 'rc-group';
      const h = document.createElement('h2');
      h.className = 'rc-group-title';
      h.textContent = g.title;
      head.append(h);
      rows.push(head);
      for (const l of g.items) rows.push(leaderRow(l, now));
    }
    ul.replaceChildren(...rows);
    $('leaders-empty').hidden = st.leaders.length > 0;
  }
  function leaderRow(l, now) {
    const li = document.createElement('li');
    const b = document.createElement('button');
    b.type = 'button';
    b.className = `rc-leader rc-st-${RC_RANK[l.state] !== undefined ? l.state : 'other'}${l.read_only ? ' rc-ro' : ''}`;
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
    if (l.read_only) {
      const ro = document.createElement('span');
      ro.className = 'rc-leader-ro';
      ro.textContent = rcReadOnlyShort(l);
      b.append(ro);
    }
    b.addEventListener('click', () => openLeader(l.sandbox));
    li.append(b);
    return li;
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
    // A read-only entry is not interrupted from the phone: the daemon
    // cannot tell which session the Escape would reach (it refuses too).
    const canStop = l?.state === 'working' && !l.read_only;
    $('interrupt').hidden = !canStop;
    if (!canStop && !$('confirm').hidden) disarmInterrupt();
    // Every sandbox the phone reaches has a board (tickets only without a team).
    $('views').hidden = !l;
    if (!l && st.screen === 's-chat' && view().mode === 'kanban') setView('chat');
    renderCtl(l);
    gateSend();
  }
  // ── Model and lead mode ──
  // The list comes once from /r/models; the select shows what the session
  // reported. A change is one call; the page shows the daemon's answer.
  async function loadModels() {
    if (st.modelsAsked) return;
    st.modelsAsked = true;
    const res = await api.models();
    st.models = res.ok ? rcModels(res.data) : [];
    if (!st.models.length) st.modelsAsked = false;
    renderCtl(leader());
  }
  async function loadSkills() {
    if (st.skillsAsked) return;
    st.skillsAsked = true;
    const res = await api.skills();
    st.skills = res.ok ? rcSkills(res.data) : [];
    if (!st.skills.length) st.skillsAsked = false;
    renderSkillList();
    renderCtl(leader());
  }
  function renderCtl(l) {
    const box = $('ctl');
    box.hidden = !l || st.screen !== 's-chat';
    if (box.hidden) return;
    const sel = $('model');
    if (sel.options.length !== st.models.length + 1 || sel.dataset.n !== String(st.models.length)) {
      sel.replaceChildren(new Option('Model…', '', true, true));
      sel.options[0].disabled = true;
      for (const m of st.models) sel.add(new Option(m.label, m.id));
      sel.dataset.n = String(st.models.length);
    }
    sel.value = rcModelValue(l, st.models);
    sel.hidden = !st.models.length;
    $('model').closest('label').hidden = !st.models.length;
    const busyModel = !!rcCtlGate(l, 'model') || st.ctlBusy;
    sel.disabled = busyModel;
    const sw = $('lead');
    sw.setAttribute('aria-checked', String(l.kind === 'leader'));
    sw.disabled = !!rcCtlGate(l, 'lead') || st.ctlBusy;
    const why = rcCtlGate(l, 'model') || rcCtlGate(l, 'lead');
    sel.title = sw.title = why;
  }
  // A skill line: the name of the list and its text. The daemon types
  // `/<name> <text>`; the page never does. Returns true when it handled it.
  async function runSkillLine(sandbox, line) {
    if (st.ctlBusy) return;
    const l = leader();
    const why = rcCtlGate(l, 'model') || rcSendGate(l);
    if (why) { note(why, 'warn'); return; }
    const raw = field.value;
    st.ctlBusy = true; renderCtl(l);
    const res = await api.runSkill(sandbox, line.name, line.args);
    st.ctlBusy = false;
    if (res.ok && st.current === sandbox && field.value === raw) {
      field.value = '';
      field.dispatchEvent(new Event('input'));
    }
    note(res.ok ? `Started /${line.name}.` : rcFailText('Skill', res), res.ok ? 'ok' : 'err');
    renderCtl(leader());
  }
  // The "/" list: tap a name to put `/<name> ` in the field.
  const skillList = $('skill-list');
  function renderSkillList() {
    const names = rcSkillMatches(field.value, st.skills);
    skillList.hidden = !names.length || st.screen !== 's-chat';
    skillList.replaceChildren();
    if (skillList.hidden) return;
    for (const n of names) {
      const li = document.createElement('li');
      li.setAttribute('role', 'presentation');
      const b = document.createElement('button');
      b.type = 'button';
      b.setAttribute('role', 'option');
      b.textContent = `/${n}`;
      // Keep the field focused (and the keyboard up) while tapping.
      b.addEventListener('pointerdown', e => e.preventDefault());
      b.addEventListener('click', () => {
        field.value = rcSkillInsert(n);
        field.dispatchEvent(new Event('input'));
        field.focus();
        field.setSelectionRange(field.value.length, field.value.length);
      });
      li.append(b);
      skillList.append(li);
    }
  }
  $('model').addEventListener('change', async () => {
    const sandbox = st.current;
    const id = $('model').value;
    if (!sandbox || !id || st.ctlBusy) return;
    st.ctlBusy = true; renderCtl(leader());
    const res = await api.setModel(sandbox, id);
    st.ctlBusy = false;
    note(res.ok ? `Model set to ${st.models.find(m => m.id === id)?.label || id}.` : rcFailText('Model change', res), res.ok ? 'ok' : 'err');
    renderCtl(leader());
  });
  // Turning lead mode ON types /lead into the session and installs its tools:
  // it asks the second tap that Interrupt asks. Turning it off does too.
  $('lead').addEventListener('click', async () => {
    const sandbox = st.current;
    const l = leader();
    if (!sandbox || !l || st.ctlBusy) return;
    const sw = $('lead');
    const on = l.kind !== 'leader';
    if (st.leadArm !== on) {
      st.leadArm = on;
      note(`${on ? 'Turn lead mode on' : 'End lead mode'} for ${l.name}? Tap Lead again to confirm.`, 'ok');
      setTimeout(() => { if (st.leadArm === on) { st.leadArm = null; note('', 'ok'); } }, 6000);
      return;
    }
    st.leadArm = null;
    st.ctlBusy = true; renderCtl(l);
    const res = await api.setLead(sandbox, on);
    st.ctlBusy = false;
    note(res.ok ? (on ? 'Lead mode is on.' : 'Lead mode ended.') : rcFailText('Lead mode change', res), res.ok ? 'ok' : 'err');
    sw.blur();
    renderCtl(leader());
  });

  // Send waits while a file uploads, while the recorder runs, on a read-only
  // entry; the typed text and the files stay where they are. While the
  // session works Send goes, and the status line says Claude Code queues it.
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
      quiet: !!rec.quiet,
      queued: st.current ? rcSendNote(leader()) : '',
    });
    const el = $('status');
    el.textContent = s.text;
    el.className = `rc-status rc-t-${s.tone}`;
  }

  async function openLeader(sandbox) {
    st.current = sandbox;
    st.loaded = null;
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
    setView(leader()?.kind === 'sandbox' ? 'chat' : view().mode);
    const res = await api.history(sandbox);
    if (st.current !== sandbox) return;
    if (!res.ok) {
      $('log-top').textContent = res.status === 404 ? 'This sandbox is not running any more.' : 'Could not load the conversation.';
      return;
    }
    const page = rcPage(res.data);
    st.items = rcMerge([], page.items);
    st.nextBefore = page.next_before;
    st.latestSeq = Math.max(page.latest_seq, ...st.items.map(i => Number(i.seq)));
    renderRows();
    st.shownSeq = st.latestSeq;
    scrollToEnd();
    st.loaded = sandbox;
    // A nudge that came while the first page loaded: read past it now.
    if (st.newerAgain) { st.newerAgain = false; fetchNewer(); }
  }

  // One ?after= read at a time; a nudge meanwhile reads once more after it.
  // A read the phone never got an answer to (its screen went off with the
  // request in the air) stops holding the others back after RC_STUCK_MS.
  async function fetchNewer() {
    const sandbox = st.current;
    if (!sandbox) return;
    if (st.loaded !== sandbox || (st.newer && Date.now() - st.newer.at < RC_STUCK_MS)) { st.newerAgain = true; return; }
    const log = $('log');
    const atEnd = log.scrollHeight - log.scrollTop - log.clientHeight < 80;
    const mine = { at: Date.now() };
    st.newer = mine;
    st.newerAgain = false;
    const res = await api.history(sandbox, { after: st.latestSeq, limit: 200 });
    if (st.newer !== mine) return;
    st.newer = null;
    if (st.current !== sandbox) return;
    if (res.ok) {
      const page = rcPage(res.data);
      if (page.items.length) {
        st.items = rcMerge(st.items, page.items);
        st.latestSeq = Math.max(st.latestSeq, page.latest_seq, ...page.items.map(i => Number(i.seq) || 0));
        renderRows();
        st.shownSeq = Math.max(st.shownSeq, st.latestSeq);
        if (atEnd) scrollToEnd(); else { st.unseenBelow = true; updateToEnd(); }
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
    updateToEnd();
  }
  $('log').addEventListener('scroll', () => { if ($('log').scrollTop < 120) fetchOlder(); updateToEnd(); }, { passive: true });

  function scrollToEnd() {
    const log = $('log');
    log.scrollTop = log.scrollHeight;
    updateToEnd();
  }
  // The round button over the composer: only in the Conversation view, only
  // while the log is away from its end; a dot when rows came in meanwhile.
  function updateToEnd() {
    const log = $('log');
    const away = st.screen === 's-chat' && !log.hidden && rcAwayFromEnd(log.scrollHeight, log.scrollTop, log.clientHeight);
    if (!away) st.unseenBelow = false;
    $('to-end').hidden = !away;
    $('to-end-dot').hidden = !(away && st.unseenBelow);
  }
  $('to-end').addEventListener('click', () => {
    const log = $('log');
    const smooth = !(reducedMotion && reducedMotion.matches);
    if (smooth && log.scrollTo) log.scrollTo({ top: log.scrollHeight, behavior: 'smooth' });
    else log.scrollTop = log.scrollHeight;
    st.unseenBelow = false;
    updateToEnd();
  });

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
  // A multi-select step (`multi`) takes several options, as checkboxes.
  let picks = { key: '', picked: [] };
  function questionCard(l, steps) {
    const key = `${l.sandbox}\n${JSON.stringify(steps)}`;
    if (picks.key !== key) picks = { key, picked: steps.map(() => []) };
    const card = document.createElement('div');
    card.className = 'rc-card-q';
    const submit = document.createElement('button');
    submit.type = 'button';
    submit.className = 'rc-btn rc-primary';
    submit.textContent = 'Send answer';
    const status = document.createElement('p');
    status.className = 'rc-q-hint';
    status.hidden = true;
    const ready = () => { submit.disabled = picks.picked.some(p => !p.length); };
    steps.forEach((step, si) => {
      const p = document.createElement('p');
      p.textContent = step.text;
      card.append(p);
      if (step.multi) {
        const hint = document.createElement('p');
        hint.className = 'rc-q-hint';
        hint.textContent = 'Pick one or more.';
        card.append(hint);
      }
      const opts = document.createElement('div');
      opts.className = 'rc-opts';
      const paint = () => opts.querySelectorAll('.rc-opt').forEach((x, xi) => {
        const on = picks.picked[si].includes(xi);
        x.classList.toggle('rc-picked', on);
        x.setAttribute('aria-pressed', String(on));
      });
      step.options.forEach((label, oi) => {
        const b = document.createElement('button');
        b.type = 'button';
        b.className = step.multi ? 'rc-opt rc-check' : 'rc-opt';
        b.textContent = String(label);
        b.addEventListener('click', () => {
          picks.picked[si] = rcAnswerPick(picks.picked[si], step, oi);
          paint();
          ready();
        });
        opts.append(b);
      });
      paint();
      card.append(opts);
    });
    submit.addEventListener('click', async () => {
      if (submit.disabled) return;
      const sandbox = l.sandbox;
      card.querySelectorAll('button').forEach(b => { b.disabled = true; });
      status.textContent = 'Sending…';
      status.hidden = false;
      const res = await api.answer(sandbox, rcAnswers(picks.picked));
      if (res.ok) {
        status.textContent = 'Answer sent.';
        submit.hidden = true;
        return;
      }
      status.textContent = rcAnswerFail(res);
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
      const why = field.value.trim() ? rcMessageCheck(field.value, st.skills) : null;
      // The typing check's own warning comes and goes with the text; a
      // refusal from the daemon stays until the next action.
      const typed = st.note && (st.note.text === RC_MSG_REASON.command || st.note.text === RC_MSG_REASON.control);
      if (why === 'command' || why === 'control') note(RC_MSG_REASON[why], 'warn');
      else if (typed) note('');
    }
    renderSkillList();
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
    // A known skill goes to the skill route; any other "/" line is refused here.
    const skill = rcSkillLine(text, st.skills);
    if (skill && !ids.length) { runSkillLine(sandbox, skill); return; }
    if (skill) { note('A skill takes text only. Remove the files, then Send.', 'warn'); return; }
    if (rcMessageCheck(text, st.skills) === 'command') { note(RC_MSG_REASON.command, 'warn'); return; }
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
  // One tray per leader for the composer and one for the ticket form: the
  // same upload, the same checks, the same ids; only the drawing differs.
  function trayIn(map, sandbox, redraw) {
    if (!sandbox) return null;
    let t = map.get(sandbox);
    if (!t) {
      t = rcTray({
        upload: (file, onProgress, signal) => rcUploadXhr(XMLHttpRequest, sandbox, file, onProgress, signal),
        // A file removed before sending leaves the Mac too (best effort:
        // unsent files expire there by themselves).
        drop: id => { api.dropUpload(sandbox, id); },
        onChange: () => { if (st.current === sandbox) redraw(); },
      });
      map.set(sandbox, t);
    }
    return t;
  }
  function tray(sandbox = st.current) { return trayIn(st.trays, sandbox, renderTray); }
  function kbTray(sandbox = st.current) { return trayIn(st.kbTrays, sandbox, renderKbTray); }
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
    if (!drawTray(tray(), { box: $('tray'), ul: $('chips'), held: $('held'), why: $('held-why') })) return;
    const t = tray();
    // A check's refusal ("Not attached: ...") is said once, in the status line.
    if (t.note) { const n = t.note; t.note = ''; note(n, 'err'); }
    gateSend();
  }
  // The chips and the held block of a tray, drawn into `d`'s elements.
  function drawTray(t, d) {
    const { box, ul, held } = d;
    if (!t) { box.hidden = true; ul.replaceChildren(); return false; }
    // The held files, waiting for the tick: nothing of them is sent yet.
    held.hidden = !t.held;
    if (t.held) d.why.textContent = t.held.reasons.join('\n');
    ul.replaceChildren(...t.chips.map(c => {
      const li = document.createElement('li');
      li.className = `rc-fchip rc-f-${c.state}`;
      li.dataset.state = c.state;
      const ic = document.createElement('span');
      ic.className = 'rc-fchip-icon';
      ic.append(icon(ICON_FILE));
      const tx = document.createElement('span');
      tx.className = 'rc-fchip-text';
      const name = dataEl('span', 'rc-fchip-name', chatShortName(c.name));
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
    return true;
  }

  // ── Dictation: a live wave while it records, then into the field ──
  // Never silent: every end or failure of a dictation puts one sentence in
  // the status line (RC_DICT_TEXT) and the control returns to idle; the UI
  // shows "recording" or "listening" only while something records or listens.
  const mic = $('mic');
  const composer = $('composer');
  const rec = { state: 'idle', stream: null, recorder: null, chunks: [], timer: null, started: 0, audio: null, analyser: null, frame: 0, levels: [], mode: 'pulse', discard: false, quiet: false, heardAt: 0, why: '' };
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
  // About 3 s of a level at zero, or of no word from the recogniser: said.
  const QUIET_MS = 3000;
  const HEARD_QUIET_MS = 8000;
  function setRec(state) {
    rec.state = state;
    if (state === 'idle') { rec.quiet = false; $('rec').dataset.quiet = 'false'; $('rec').dataset.hear = 'off'; }
    composer.dataset.rec = state;
    $('rec').hidden = state === 'idle';
    const on = state === 'recording' || state === 'listening';
    mic.setAttribute('aria-label', on ? 'Stop dictation' : micWhy ? `Dictate (unavailable: ${micWhy})` : 'Dictate');
    mic.setAttribute('aria-pressed', String(on));
    mic.disabled = state === 'transcribing';
    gateSend();
  }
  function setQuiet(q) {
    if (rec.quiet === q) return;
    rec.quiet = q;
    $('rec').dataset.quiet = String(q);
    renderStatus();
  }
  // The dictation's outcome, in the status line. A warning keeps its tone.
  function say(text, tone = 'info') {
    st.hint = text ? { text, tone } : '';
    renderStatus();
  }
  mic.addEventListener('click', () => {
    if (rec.state === 'listening') { speech.stop('person'); return; }
    if (rec.state === 'transcribing' || rec.starting) return;
    if (rec.recorder) { stopRecording(false); return; }
    // iPhone dictation, when chosen and available at this tap; else the Mac.
    if (prefs.dictation() === RC_DICTATION.iphone && !rec.recorder && speech.start()) return;
    macRecord();
  });
  async function macRecord() {
    if (micWhy) { say(micWhy, 'warn'); return; }
    rec.starting = true;
    let stream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
    } catch (e) {
      rec.starting = false;
      say(e && e.name === 'NotAllowedError'
        ? 'Microphone refused. Allow it for this site in Settings, then try again.'
        : e && e.name === 'NotFoundError' ? 'No microphone found.'
          : e && e.name === 'NotReadableError' ? RC_DICT_TEXT.taken : 'The microphone could not start. Tap it to try again.', 'warn');
      return;
    }
    rec.starting = false;
    const release = () => stream.getTracks().forEach(t => t.stop());
    if (!st.current || document.visibilityState === 'hidden') { release(); return; }
    // The first format this browser records (audio/mp4 on an iPhone).
    let recorder;
    try {
      const mimeType = rcRecMime(t => MediaRecorder.isTypeSupported?.(t));
      recorder = mimeType ? new MediaRecorder(stream, { mimeType }) : new MediaRecorder(stream);
    } catch (_) {
      try { recorder = new MediaRecorder(stream); } catch (_) { recorder = null; }
    }
    if (!recorder) { release(); say('This browser cannot record audio.', 'warn'); return; }
    Object.assign(rec, { stream, recorder, chunks: [], started: Date.now(), discard: false, sandbox: st.current, why: '', heardAt: Date.now(), quiet: false });
    recorder.addEventListener('dataavailable', e => { if (e.data && e.data.size) rec.chunks.push(e.data); });
    recorder.addEventListener('stop', () => finishRecording(recorder));
    recorder.addEventListener('error', () => { if (rec.recorder === recorder) { rec.why = RC_DICT_TEXT.recFailed; stopRecording(false); } });
    // The microphone taken away (a call, another app) or gone: stop, and
    // transcribe what was recorded before, with the sentence that says why.
    for (const t of stream.getAudioTracks()) {
      const taken = () => { if (rec.recorder === recorder) { rec.why = RC_DICT_TEXT.taken; stopRecording(false); } };
      t.addEventListener('ended', taken);
      // A mute that lasts a second, not a blip.
      t.addEventListener('mute', () => setTimeout(() => { if (t.muted) taken(); }, 1000));
    }
    try {
      recorder.start();
    } catch (_) {
      release();
      Object.assign(rec, { stream: null, recorder: null, chunks: [] });
      say('The recording could not start. Tap the microphone to try again.', 'warn');
      return;
    }
    st.hint = '';
    // A warning stays, and so does the sentence that says why the Mac records.
    if (st.note?.tone !== 'warn' && st.note?.text !== RC_SPEECH_FALLBACK_TEXT) st.note = null;
    startWave(stream);
    setRec('recording');
    const tick = () => {
      const s = Math.floor((Date.now() - rec.started) / 1000);
      $('rec-time').textContent = rcClock(s);
      if (s >= RC_VOICE_MAX_S && rec.recorder) stopRecording(false);
    };
    tick();
    rec.timer = setInterval(tick, 250);
  }

  // ── iPhone dictation (Web Speech API), chosen in the menu ──
  // The page opens no audio stream of its own here (a second user of the
  // microphone could take it from the recogniser): no wave, but a dot that
  // turns live when the recogniser says it hears sound, and the words as they
  // come. The final words go into the field at the caret; interim ones are
  // shown lighter. A refusal of the browser before any word falls back to
  // the Mac for this recording, with one sentence. A recogniser that ends by
  // itself (a pause, its own time limit) starts again at most twice
  // (rcSpeechRestart); every other end says why (rcSpeechEnd).
  const lastCaret = { value: null, at: 0 };
  field.addEventListener('blur', () => { lastCaret.value = field.value; lastCaret.at = field.selectionStart; });
  // The preview: the END of what is said, as much as two lines hold.
  function showPreview(final, interim) {
    const box = $('rec-words');
    for (let max = 160; ; max -= 12) {
      const t = rcTail(final, interim, max);
      $('rec-final').textContent = t.final;
      $('rec-interim').textContent = t.interim;
      if (box.scrollHeight <= box.clientHeight + 1 || max <= 24) return;
    }
  }
  const speech = {
    r: null, base: '', caret: 0, done: '', final: '', carry: '', heard: false, sandbox: null, fellBack: false,
    error: '', reason: '', restarts: 0, roundAt: 0, guard: 0, last: '', heardRound: false, heardAt: 0, startedOk: false,
    // Words of this dictation so far: the rounds before, then this one.
    words() { return [speech.done, speech.final].filter(Boolean).join(' '); },
    start() {
      const SR = window.SpeechRecognition || window.webkitSpeechRecognition;
      if (typeof SR !== 'function') return speech.fallback();
      // The caret: where it was when the field lost focus to the mic button,
      // if the text has not changed since; else the end.
      const caret = document.activeElement === field ? field.selectionStart : lastCaret.value === field.value ? lastCaret.at : field.value.length;
      Object.assign(speech, { base: field.value, caret, done: '', final: '', carry: '', heard: false, sandbox: st.current, fellBack: false, error: '', reason: '', restarts: 0 });
      if (!speech.round_(SR)) { speech.r = null; return speech.fallback(); }
      showPreview('', '');
      $('rec-words').hidden = false;
      $('rec-time').textContent = rcClock(0);
      $('rec').dataset.hear = 'off';
      rec.started = Date.now();
      st.hint = '';
      if (st.note?.tone !== 'warn') st.note = null;
      composer.dataset.wave = 'none';
      setRec('listening');
      clearInterval(rec.timer);
      rec.timer = setInterval(() => {
        const s = Math.floor((Date.now() - rec.started) / 1000);
        $('rec-time').textContent = rcClock(s);
        if (s >= RC_VOICE_MAX_S) speech.stop('time');
        // Nothing heard for a while after the start of a round: said.
        // After words, a longer pause: a recogniser can go deaf without any
        // event (WebKit bug 321436); the person is told, it is not ended.
        if (speech.r) setQuiet(speech.heardRound ? Date.now() - speech.heardAt >= HEARD_QUIET_MS : Date.now() - speech.roundAt >= QUIET_MS);
      }, 250);
      return true;
    },
    // One recogniser, one round. → false when it cannot start.
    round_(SR) {
      let r;
      try {
        r = new SR();
        r.lang = rcSpeechLang(navigator.language, document.documentElement.lang);
        r.interimResults = true;
        r.continuous = true;
        r.maxAlternatives = 1;
      } catch (_) { return false; }
      speech.r = r;
      speech.final = '';
      speech.heardRound = false;
      speech.startedOk = false;
      speech.roundAt = Date.now();
      r.onstart = r.onaudiostart = () => { if (speech.r === r) { speech.startedOk = true; if ($('rec').dataset.hear === 'off') $('rec').dataset.hear = 'on'; } };
      r.onsoundstart = r.onspeechstart = () => { if (speech.r === r) { speech.startedOk = true; $('rec').dataset.hear = 'sound'; } };
      r.onsoundend = () => { if (speech.r === r) $('rec').dataset.hear = 'on'; };
      r.onresult = e => {
        if (speech.r !== r) return;
        const t = rcSpeechRound(e.results, speech.carry);
        if (t.final || t.interim) { speech.heard = true; speech.heardRound = true; speech.heardAt = Date.now(); setQuiet(false); }
        speech.final = t.final;
        speech.last = t.last;
        // The field holds what is final, at the caret; the bar shows both,
        // its end in view.
        if (st.current === speech.sandbox) field.value = rcInsertAt(speech.base, speech.caret, speech.words()).value;
        showPreview(speech.words(), t.interim);
      };
      r.onerror = e => {
        if (speech.r !== r) return;
        const code = String(e && e.error || 'error');
        if (!speech.heard && rcSpeechFallback(code)) {
          speech.fellBack = true;
          speech.end();
          note(RC_SPEECH_FALLBACK_TEXT, 'info');
          macRecord();
          return;
        }
        // onend follows and says it; kept in case it does not come.
        speech.error = code;
      };
      r.onend = () => { if (speech.r === r) speech.ended(); };
      try {
        r.start();
      } catch (_) {
        r.onstart = r.onaudiostart = r.onsoundstart = r.onspeechstart = r.onsoundend = r.onresult = r.onerror = r.onend = null;
        return false;
      }
      // A recogniser that never says it started is not listening.
      clearTimeout(speech.guard);
      speech.guard = setTimeout(() => {
        if (speech.r === r && !speech.startedOk && !speech.heardRound) { speech.reason = 'nostart'; speech.abort(); }
      }, 4000);
      return true;
    },
    // The recogniser ended: by the person, or by itself.
    ended() {
      const reason = speech.reason || 'self';
      const restart = rcSpeechRestart({
        error: speech.error, reason, hidden: document.visibilityState === 'hidden', restarts: speech.restarts,
        heard: speech.heardRound, ms: Date.now() - speech.roundAt, total: Date.now() - rec.started,
      });
      if (restart && st.current === speech.sandbox) {
        const SR = window.SpeechRecognition || window.webkitSpeechRecognition;
        const old = speech.r;
        old.onstart = old.onaudiostart = old.onsoundstart = old.onspeechstart = old.onsoundend = old.onresult = old.onerror = old.onend = null;
        // What this round heard is kept; the next round's first result may
        // repeat its last words: rcSpeechRound adds only the new ones.
        speech.done = speech.words();
        speech.carry = speech.last || '';
        speech.restarts++;
        if (speech.round_(SR)) return;
        speech.r = old;
      }
      speech.end();
    },
    // Not available at this tap: say so once, and let the Mac record.
    fallback() {
      note(RC_SPEECH_FALLBACK_TEXT, 'info');
      return false;
    },
    // A stop that the recogniser does not acknowledge still ends the UI.
    stop(reason = 'person') {
      const r = speech.r;
      if (!r) return;
      speech.reason = speech.reason || reason;
      try { r.stop(); } catch (_) { speech.end(); return; }
      setTimeout(() => { if (speech.r === r) speech.end(); }, 1500);
    },
    abort(reason) {
      if (reason) speech.reason = speech.reason || reason;
      try { speech.r?.abort(); } catch (_) {}
      speech.end();
    },
    end() {
      const r = speech.r;
      if (!r) return;
      speech.r = null;
      clearTimeout(speech.guard);
      r.onstart = r.onaudiostart = r.onsoundstart = r.onspeechstart = r.onsoundend = r.onresult = r.onerror = r.onend = null;
      clearInterval(rec.timer);
      $('rec-words').hidden = true;
      $('rec-final').textContent = '';
      $('rec-interim').textContent = '';
      setRec('idle');
      if (speech.fellBack) return;
      const words = speech.words();
      const out = rcSpeechEnd({ error: speech.error, reason: speech.reason || 'self', words: !!words });
      if (st.current === speech.sandbox) {
        const put = rcInsertAt(speech.base, speech.caret, words);
        field.value = put.value;
        // The advisory checks run on dictated text as on typed text.
        field.dispatchEvent(new Event('input'));
        say(out.text, out.tone);
      } else if (speech.sandbox && words) {
        st.drafts.set(speech.sandbox, rcInsertAt(st.drafts.get(speech.sandbox) || speech.base, (st.drafts.get(speech.sandbox) || speech.base).length, words).value);
      }
    },
  };
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'hidden' && speech.r) speech.stop('hidden'); });

  // The wave: levels from an AnalyserNode on the same stream, drawn on a
  // canvas (no data: or blob: URL). Recording never depends on it: without
  // an analyser, a steady pulse; with reduced motion, a plain level meter.
  // A level at zero for QUIET_MS is said in the status line.
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
      if (level > 0) rec.heardAt = Date.now();
      if (rec.state === 'recording') setQuiet(Date.now() - rec.heardAt >= QUIET_MS);
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
    if (speech.r) { if (discard) speech.abort('hidden'); else speech.stop('person'); }
    const recorder = rec.recorder;
    if (!recorder) return;
    rec.discard = rec.discard || !!discard;
    try { recorder.stop(); } catch (_) { finishRecording(recorder); return; }
    // A recorder that never says it stopped still ends the UI.
    setTimeout(() => { if (rec.recorder === recorder) finishRecording(recorder); }, 1500);
  }
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden' && rec.recorder) stopRecording(true);
  });
  window.addEventListener('pagehide', () => stopRecording(true));

  // Once per recorder: a second call (its `stop` after a forced end) is ignored.
  async function finishRecording(recorder) {
    if (!recorder || rec.recorder !== recorder) return;
    clearInterval(rec.timer);
    stopWave();
    rec.stream?.getTracks().forEach(t => t.stop());
    const ms = Date.now() - rec.started;
    const blob = new Blob(rec.chunks, { type: recorder.mimeType || 'audio/mp4' });
    const { discard, sandbox, why } = rec;
    Object.assign(rec, { stream: null, recorder: null, chunks: [], discard: false, why: '' });
    if (discard) {
      setRec('idle');
      say('Recording stopped: the page was hidden. Nothing was sent.', 'warn');
      return;
    }
    // An empty or too short clip is not sent.
    const short = rcClipCheck(blob.size, ms);
    if (short) {
      setRec('idle');
      say(why || short, 'warn');
      return;
    }
    drawWave(true);
    setRec('transcribing');
    // The clock goes on while the Mac transcribes: progress, not a freeze.
    const t0 = Date.now();
    rec.timer = setInterval(() => { $('rec-time').textContent = rcClock((Date.now() - t0) / 1000); }, 250);
    $('rec-time').textContent = rcClock(0);
    let res, pcm = null;
    try {
      pcm = await toPcm(blob);
    } catch (_) {
      pcm = null;
    }
    if (pcm && rcSilentPcm(pcm)) res = 'silent';
    else if (pcm) {
      let slow;
      try {
        res = await Promise.race([
          api.transcribe(pcm),
          new Promise(r => { slow = setTimeout(() => r({ ok: false, timeout: true }), 120000); }),
        ]);
      } catch (_) {
        res = { ok: false, status: 0, unreachable: true };
      } finally { clearTimeout(slow); }
    }
    clearInterval(rec.timer);
    setRec('idle');
    if (st.note?.text === RC_SPEECH_FALLBACK_TEXT) st.note = null;
    if (res === 'silent') return say(why || RC_DICT_TEXT.macSilent, 'warn');
    const out = rcTranscribed(pcm ? res : null);
    if (!out.words) return say(why ? `${why} ${out.text}` : out.text, 'warn');
    if (st.current === sandbox) {
      field.value = rcWithDictation(field.value, out.words);
      field.dispatchEvent(new Event('input'));
      say(why ? `${why} ${RC_DICT_TEXT.kept}` : RC_DICT_TEXT.added, why ? 'warn' : 'ok');
    } else if (sandbox) {
      st.drafts.set(sandbox, rcWithDictation(st.drafts.get(sandbox) || '', out.words));
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
  // ── Notifications: a switch the person taps, per device (sw.js, API-push.md) ──
  // Shown only where Web Push exists; on an iPhone that means the page was
  // added to the Home Screen. The switch shows the truth: on only when the
  // Mac and this browser both hold the subscription.
  const pushNo = rcPushSupport({ serviceWorker: 'serviceWorker' in navigator, PushManager: 'PushManager' in window, Notification: 'Notification' in window }, navigator.userAgent);
  const push = pushNo ? null : rcPush({ api, sw: navigator.serviceWorker, Notif: window.Notification });
  const pushSt = { on: false, busy: false };
  function renderPush(on, text) {
    pushSt.on = on;
    const b = $('push');
    b.setAttribute('aria-checked', String(on));
    b.disabled = pushSt.busy;
    $('push-state').textContent = pushSt.busy ? 'Working…' : on ? 'On' : 'Off';
    const note = $('push-note');
    note.textContent = text || '';
    note.hidden = !text;
  }
  if (pushNo) {
    $('push').hidden = true;
    $('push-note').textContent = RC_PUSH_TEXT[pushNo];
    $('push-note').hidden = false;
  } else {
    $('push').hidden = false;
  }
  async function refreshPush() {
    if (!push || pushSt.busy) return;
    const r = await push.state();
    if (!pushSt.busy) renderPush(r.state === 'on', r.text);
  }
  if (push) $('push').addEventListener('click', async e => {
    e.stopPropagation();
    if (pushSt.busy) return;
    const turnOn = !pushSt.on;
    pushSt.busy = true;
    renderPush(pushSt.on, '');
    // The tap's own call: nothing is awaited before the worker starts.
    const r = await (turnOn ? push.enable() : push.disable());
    pushSt.busy = false;
    if (turnOn) renderPush(r.ok, r.ok ? '' : r.text);
    else renderPush(false, r.ok ? '' : r.text);
  });

  // The person is looking at the page: the icon's badge has done its job.
  const clearBadge = () => { if (document.visibilityState === 'visible') rcClearBadge(navigator); };
  document.addEventListener('visibilitychange', clearBadge);
  clearBadge();

  $('menu-btn').addEventListener('click', e => {
    e.stopPropagation();
    const open = $('menu').hidden;
    closeMenu();
    if (open) refreshPush();
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

  // ── Text that is not the page's own (security review P6) ──
  // Isolated (dir="auto", unicode-bidi: isolate) so it cannot reorder its
  // neighbours, and every invisible or bidi control drawn as a boxed code.
  function markInto(node, text) {
    for (const part of rcMarked(text)) {
      if (typeof part === 'string') node.append(part);
      else {
        const c = document.createElement('span');
        c.className = 'rc-cc';
        c.textContent = part.code;
        node.append(c);
      }
    }
    return node;
  }
  function dataEl(tag, cls, text, mono) {
    const e = document.createElement(tag);
    e.className = `rc-data${mono ? ' rc-mono' : ''}${cls ? ` ${cls}` : ''}`;
    e.dir = 'auto';
    return markInto(e, text === null || text === undefined ? '' : String(text));
  }
  // A line of the page's words and the daemon's data: [string | { data }].
  function partsInto(node, list, mono = true) {
    for (const p of list) node.append(typeof p === 'string' ? p : dataEl('span', null, p.data, mono));
    return node;
  }

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
    updateToEnd();
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
      kbStatus(res.status === 404 ? 'This team leader is no longer running.' : rcFailText('Board', res).replace('Board not sent', 'The board could not be read'));
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
    v.timer = setTimeout(() => {
      // A read that ended meanwhile may already hold the board this named.
      if (v.want && v.board?.latest === v.want) return;
      if (view().mode === 'kanban') loadBoard(); else v.board = null;
    }, 400);
  }

  // A new ticket: a small form over the board, text and files. The Mac masks the
  // text like a message and puts the ticket in Planned.
  function kbFormShow(on) {
    $('kb-form').hidden = !on;
    $('kb-newbar').hidden = on || !!view().ticket;
    if (on) { $('kb-ferr').hidden = true; $('kb-title').focus(); }
  }
  // The form's files: the composer's tray, drawn under the form.
  function renderKbTray() {
    const t = kbTray();
    if (!drawTray(t, { box: $('kb-tray'), ul: $('kb-chips'), held: $('kb-held'), why: $('kb-held-why') })) return;
    if (t.note) { const err = $('kb-ferr'); err.textContent = t.note; err.hidden = false; t.note = ''; }
    $('kb-create').disabled = t.pending() > 0;
  }
  $('kb-attach').addEventListener('click', () => { $('kb-file').value = ''; $('kb-file').click(); });
  $('kb-file').addEventListener('change', () => {
    const t = kbTray();
    const files = [...($('kb-file').files || [])];
    $('kb-file').value = '';
    if (!t || !files.length) return;
    $('kb-ferr').hidden = true;
    $('kb-tick').checked = false;
    t.offer(files);
  });
  $('kb-held-go').addEventListener('click', () => { const t = kbTray(); if (t) t.confirmHeld($('kb-tick').checked === true); });
  $('kb-held-cancel').addEventListener('click', () => { const t = kbTray(); if (t) { t.note = ''; t.discardHeld(); } });
  $('kb-new').addEventListener('click', () => { kbFormShow(true); renderKbTray(); });
  $('kb-cancel').addEventListener('click', () => { $('kb-form').reset(); const t = kbTray(); if (t) t.clear(); kbFormShow(false); });
  $('kb-form').addEventListener('submit', async e => {
    e.preventDefault();
    const sandbox = st.current;
    const title = $('kb-title').value.trim();
    const err = $('kb-ferr');
    if (!title) { err.textContent = 'A ticket needs a title.'; err.hidden = false; $('kb-title').focus(); return; }
    const t = kbTray(sandbox);
    if (t && t.failed()) { err.textContent = 'A file did not upload: retry or remove it.'; err.hidden = false; return; }
    const ids = t ? t.ids() : [];
    $('kb-create').disabled = true;
    const res = await api.createTicket(sandbox, title, $('kb-desc').value.trim(), ids);
    $('kb-create').disabled = false;
    if (!res.ok) {
      err.textContent = res.status === 404 ? 'This team leader is no longer running.' : rcFailText('Ticket', res);
      err.hidden = false;
      return;
    }
    $('kb-form').reset();
    if (t) t.sent(ids);
    kbFormShow(false);
    if (st.current !== sandbox) return;
    const v = view(sandbox);
    if (v.board?.columns.some(c => c.id === 'planned')) v.col = 'planned';
    loadBoard();
  });

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
    $('kb-newbar').hidden = !!v.ticket || !$('kb-form').hidden;
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
    top.append(dataEl('span', 'rc-kb-num', c.num));
    if (c.priority !== 'none') top.append(el('span', `rc-kb-prio rc-kb-${c.priority}`, c.priority.toUpperCase()));
    if (c.age) top.append(dataEl('span', 'rc-kb-age', c.age));
    b.append(top, dataEl('span', 'rc-kb-title', c.title || c.num));
    if (c.marks.length) {
      const m = el('span', 'rc-kb-marks');
      for (const x of c.marks) m.append(dataEl('span', `rc-kb-mark rc-tone-${x.tone}`, x.label));
      b.append(m);
    }
    if (c.thread) {
      const t = el('span', `rc-kb-thread rc-tone-${c.thread_tone}`);
      t.append(el('span', 'rc-kb-dot'), dataEl('span', 'rc-kb-tname', c.thread));
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
    head_.append(dataEl('span', 'rc-kb-num', t?.num || card.num));
    const prio = t?.priority || card.priority;
    if (prio !== 'none') head_.append(el('span', `rc-kb-prio rc-kb-${prio}`, RC_KB_PRIORITY[prio]));
    const col = view().board?.columns.find(c => c.id === (t?.column || ''))?.title;
    if (col) head_.append(el('span', 'rc-kb-tcol', col));
    frag.append(head_, dataEl('h2', 'rc-kb-ttitle', t?.title || card.title));
    if (card.marks.length) {
      const m = el('div', 'rc-kb-marks');
      for (const x of card.marks) m.append(dataEl('span', `rc-kb-mark rc-tone-${x.tone}`, x.label));
      frag.append(m);
    }
    if (card.thread) {
      const th = el('p', `rc-kb-thread rc-tone-${card.thread_tone}`);
      th.append(el('span', 'rc-kb-dot'), dataEl('span', 'rc-kb-tname', card.thread));
      if (card.thread_state) th.append(el('span', 'rc-kb-tstate', RC_KB_THREAD[card.thread_state] || card.thread_state));
      frag.append(th);
    }
    if (note) frag.append(el('p', 'rc-item-note', note));
    if (!card.n) frag.append(el('p', 'rc-muted rc-small-text', 'A thread of the plan, with no ticket of its own.'));
    if (t) {
      if (t.description) frag.append(dataEl('p', 'rc-kb-desc', t.description));
      if (t.files.length) {
        const box = el('div', 'rc-kv');
        box.append(el('p', 'rc-kv-k', t.files.length === 1 ? '1 file' : `${t.files.length} files`));
        const ul = el('ul', 'rc-kb-files');
        for (const f of t.files) {
          const li = el('li');
          li.append(dataEl('span', 'rc-file-path', f.name), el('span', 'rc-file-n', chatSize(f.size)));
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
          li.append(dataEl('span', 'rc-kb-when', h.when), dataEl('span', null, h.text));
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
    // The number is plans and merges; network requests raise a dot instead.
    const { n, dot } = st.l2?.level2 ? rcApBadge(st.l2.counts) : { n: 0, dot: false };
    const b = $('badge');
    b.textContent = n > 99 ? '99+' : String(n);
    b.hidden = !n;
    $('badge-dot').hidden = !dot;
    $('approvals-btn').setAttribute('aria-label', n ? `Approvals, ${n} waiting${dot ? '. Network requests wait' : ''}` : dot ? 'Approvals. Network requests wait' : 'Approvals');
    apToastCheck(n);
    // Approvals is open and the counts moved: re-read its lists (the filter's
    // entries and counts follow; the choice and the scroll are kept).
    if (st.screen === 's-approvals' && !st.apLoading && st.l2?.level2 && apCountsKey() !== st.apSeen) { st.apLoading = true; loadApLists(); }
  }
  // `filter`: undefined (the person's own choice stands), null (All) or a sandbox.
  function openApprovals(filter) {
    st.l2From = st.screen === 's-chat' ? st.current : null;
    if (st.screen === 's-chat') { saveDraft(); stopRecording(false); disarmInterrupt(); }
    if (filter !== undefined) { st.apTouched = true; st.apFilter = filter; }
    showApprovals();
  }
  $('approvals-btn').addEventListener('click', () => openApprovals());

  // ── A plan or merge went up: one toast, never a plan's text ──
  // From the counts (the `approvals` event or a refresh). The first count the
  // page sees is the baseline; going down, or network and stopped going up,
  // say nothing. The sandbox's name comes from the lists the page already
  // reads (plans, merges), shown as text.
  async function apSeen() {
    const got = await Promise.all([l2.list('plans'), l2.list('merges')]);
    const fresh = [];
    ['plans', 'merges'].forEach((k, i) => {
      const r = got[i];
      for (const it of r.ok && r.data && Array.isArray(r.data[k]) ? r.data[k] : []) {
        const id = str(it?.id);
        if (!id || st.apIds.has(id)) continue;
        st.apIds.add(id);
        fresh.push({ what: k, sandbox: str(it.sandbox) });
      }
    });
    return fresh;
  }
  async function apToastCheck(n) {
    const base = st.apBase;
    st.apBase = n;
    if (base === null) { if (n) apSeen(); return; }
    if (n <= base) return;
    const fresh = await apSeen();
    if (!['s-leaders', 's-chat', 's-item'].includes(st.screen) || sheet) return;
    const delta = n - base;
    const merges = fresh.length > 0 && fresh.every(f => f.what === 'merges');
    const noun = merges ? 'merge' : 'plan';
    const sbs = new Set(fresh.map(f => f.sandbox).filter(Boolean));
    const sb = sbs.size === 1 && fresh.every(f => f.sandbox) ? [...sbs][0] : '';
    const name = sb ? (st.leaders.find(l => l.sandbox === sb)?.name || sb) : '';
    $('ap-toast-text').textContent = delta > 1 ? `${delta} ${fresh.length ? noun : 'plan'}s wait for you` : `A ${noun} waits for you`;
    $('ap-toast-sb').textContent = name;
    $('ap-toast-sb').hidden = !name;
    st.apToastTo = sb || null;
    $('ap-toast').hidden = false;
    clearTimeout(st.apToastTimer);
    st.apToastTimer = setTimeout(hideApToast, 6000);
  }
  function hideApToast() {
    clearTimeout(st.apToastTimer);
    $('ap-toast').hidden = true;
  }
  $('ap-toast-go').addEventListener('click', () => { const to = st.apToastTo; hideApToast(); openApprovals(to); });
  $('ap-toast-x').addEventListener('click', hideApToast);
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
    // Untouched, the filter starts again from the leader being looked at.
    if (!st.apTouched) { st.apFilter = undefined; st.apWanted = st.l2From; }
    st.apLoading = true;
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
    if (!a?.level2) { st.apLoading = false; $('ap-lists').replaceChildren(); $('ap-filter').hidden = true; $('ap-none').hidden = true; $('ap-empty').hidden = true; return; }
    await loadApLists();
  }
  // Read the four lists, then draw them. Also the live path: a change of the
  // counts re-reads them (nothing is added to what the page asks).
  async function loadApLists() {
    const seq = ++st.apSeq;
    const got = await Promise.all(LISTS.map(L => l2.list(L.what)));
    if (st.screen !== 's-approvals' || seq !== st.apSeq) return;
    st.apLists = got;
    for (const k of ['plans', 'merges']) for (const it of apItems()[k]) if (it?.id) st.apIds.add(str(it.id));
    st.apSeen = apCountsKey();
    st.apLoading = false;
    renderApLists();
  }
  function apCountsKey() {
    const c = st.l2?.counts;
    return c && typeof c === 'object' ? LISTS.map(L => Number(c[L.count]) || 0).join(',') : '';
  }
  // The items of each list as the Mac sent them (a failed list: none).
  function apItems() {
    const out = {};
    LISTS.forEach((L, i) => {
      const r = st.apLists[i];
      out[L.what] = r && r.ok && r.data && Array.isArray(r.data[L.key]) ? r.data[L.key] : [];
    });
    return out;
  }
  function apCounts(all) {
    return { plans: all.plans.length, merges: all.merges.length, network: all.network.length };
  }
  // Display filter only: it hides rows, nothing else. The detail, the
  // challenge and the action still come from the Mac, per item.
  function renderApLists() {
    const all = apItems();
    const entries = rcApEntries(all, st.leaders, st.apFilter);
    if (st.apFilter === undefined) st.apFilter = rcApDefault(st.apWanted, entries);
    const only = st.apFilter;
    const scroll = $('s-approvals').scrollTop;
    const stripLeft = $('ap-filter').scrollLeft;
    const total = LISTS.reduce((t, L) => t + all[L.what].length, 0);
    // The strip is for choosing between sandboxes: nothing to choose from
    // when at most one has something and no choice was made.
    const showStrip = total > 0 && (entries.length > 1 || only);
    $('ap-filter').hidden = !showStrip;
    if (showStrip) {
      const mk = (label, count, sandbox) => {
        const t = el('button', 'rc-kb-tab rc-ap-fchip');
        t.type = 'button';
        t.setAttribute('role', 'tab');
        t.setAttribute('aria-selected', String(sandbox === only || (!sandbox && !only)));
        if (sandbox) t.dataset.sandbox = sandbox;
        t.append(el('span', 'rc-ap-fname', label));
        if (count.n) t.append(el('span', 'rc-kb-count', count.n));
        if (count.dot) { const d = el('span', 'rc-ap-dot'); d.setAttribute('role', 'img'); d.setAttribute('aria-label', 'Network requests wait'); t.append(d); }
        t.addEventListener('click', () => { st.apTouched = true; st.apFilter = sandbox; renderApLists(); });
        return t;
      };
      $('ap-filter').replaceChildren(mk('All sandboxes', rcApBadge(apCounts(all)), null), ...entries.map(e => mk(e.name, e, e.sandbox)));
      $('ap-filter').scrollLeft = stripLeft;
      if (!stripLeft) $('ap-filter').querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
    } else {
      $('ap-filter').replaceChildren();
    }
    const sections = [];
    LISTS.forEach((L, i) => {
      const r = st.apLists[i];
      // A list the Mac could not read says so, quietly, in its place.
      if (r && !r.ok && r.status !== 403) {
        const sec = el('section', 'rc-ap-sec');
        sec.append(el('h2', 'rc-ap-h', L.title), el('p', 'rc-muted rc-small-text rc-ap-fail', rcL2Fail(r)));
        sections.push(sec);
        return;
      }
      const items = rcApFilter(L.what, all[L.what], only);
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
    // Nothing in the chosen sandbox: say so, with the way back.
    const goneName = entries.find(e => e.sandbox === only)?.name || only;
    $('ap-none-text').textContent = only ? `Nothing waits in ${goneName}` : '';
    $('ap-none').hidden = !(only && !sections.length);
    $('ap-empty').hidden = sections.length > 0 || !!only;
    $('s-approvals').scrollTop = scroll;
  }
  $('ap-none-all').addEventListener('click', () => { st.apTouched = true; st.apFilter = null; renderApLists(); });
  function listRow(what, it) {
    const li = el('li');
    const b = el('button', `rc-ap-row rc-ap-${what}`);
    b.type = 'button';
    let id, main, meta;
    if (what === 'plans') { id = str(it.id); main = str(it.summary) || id; meta = `${str(it.sandbox)} · ${Number(it.threads) || 0} threads`; }
    else if (what === 'merges') { id = str(it.id); main = `Merge of ${str(it.sandbox)}`; meta = `${str(it.variant)} · ${Number(it.branches) || 0} branches`; }
    else if (what === 'network') { id = str(it.id); main = str(it.host); meta = str(it.sandbox); }
    else { id = str(it.name); main = str(it.name); meta = [str(it.agent) || (it.chat ? 'chat' : ''), str(it.workspace)].filter(Boolean).join(' · '); }
    b.append(dataEl('span', 'rc-ap-main', main, what === 'network'), dataEl('span', 'rc-ap-meta', meta));
    b.addEventListener('click', () => openItem(what, id, '', what === 'network' ? { sandbox: str(it.sandbox), host: str(it.host) } : null));
    li.append(b);
    return li;
  }

  // ── A detail ──
  async function openItem(what, id, said, hint) {
    closeSheet();
    st.item = { what, id, data: null, digest: '', models: [], picked: {} };
    const titles = { plans: 'Plan', merges: 'Merge', network: 'Network request', stopped: 'Stopped sandbox' };
    setTitle(titles[what], '');
    show('s-item');
    itemStatus(said || '');
    $('item').replaceChildren(el('p', 'rc-muted', 'Loading…'));
    $('item-acts').replaceChildren();
    const res = await l2.detail(what, id);
    if (st.item?.what !== what || st.item.id !== id) return;
    if (!res.ok || !res.data) {
      // sbx may issue a pending request again under a new id (the sandbox
      // retries); the list in hand then names an id that is gone. Read the
      // list once more and, for the same sandbox and host, open the new id.
      // The detail, its digest and Face ID are the new item's own.
      if (res.status === 404 && what === 'network' && hint) {
        const fresh = await l2.list('network');
        if (st.item?.what !== what || st.item.id !== id) return;
        const rows = fresh.ok && Array.isArray(fresh.data?.requests) ? fresh.data.requests : [];
        const again = rows.find(r => r && str(r.id) !== id && str(r.sandbox) === hint.sandbox && str(r.host) === hint.host);
        if (again) return openItem(what, str(again.id), said, null);
      }
      $('item').replaceChildren(el('p', 'rc-muted', res.status === 404 ? 'This is not waiting any more.' : rcL2Fail(res)));
      return;
    }
    const d = res.data;
    const key = { plans: 'plan', merges: 'merge', network: 'request', stopped: 'sandbox' }[what];
    const data = d[key] && typeof d[key] === 'object' ? d[key] : {};
    st.item.data = data;
    st.item.digest = typeof d.digest === 'string' && /^[0-9a-f]{64}$/.test(d.digest) ? d.digest : '';
    // The models a thread may be switched to (the configured list, ids and labels).
    st.item.models = what === 'plans' && Array.isArray(d.models) ? d.models.filter(m => m && typeof m.id === 'string' && m.id).map(m => ({ id: m.id, label: str(m.label) || m.id })) : [];
    const body = what === 'plans' ? planView(data) : what === 'merges' ? mergeView(data) : what === 'network' ? networkView(data) : stoppedView(data, d.confirm);
    $('item').replaceChildren(...body);
    renderActs(what, data);
  }
  // A line above the detail: a string, or parts ([string | { data }]).
  function itemStatus(text, tone) {
    const p = $('item-status');
    p.replaceChildren();
    if (Array.isArray(text)) partsInto(p, text, false); else p.textContent = text;
    p.className = `rc-item-status${tone ? ` rc-t-${tone}` : ''}`;
    p.hidden = !(Array.isArray(text) ? text.length : text);
  }
  // A field: a quiet label and its value, as text. Long text folds.
  function field_(label, value, opts = {}) {
    const box = el('div', `rc-kv${opts.mono ? ' rc-kv-mono' : ''}`);
    box.append(el('p', 'rc-kv-k', label));
    const v = dataEl('p', 'rc-kv-v', value, opts.mono);
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
  // A detail's head: its title (data) and a line of parts ([string | { data }]).
  function head(title, sub) {
    const h = el('div', 'rc-item-head');
    h.append(dataEl('h2', 'rc-item-title', title));
    if (sub && sub.length) h.append(partsInto(el('p', 'rc-item-sub'), sub));
    return h;
  }
  function planView(p) {
    const out = [head(str(p.summary) || 'A plan', [{ data: str(p.sandbox) }, ` · ${str(p.variant) === 'shared' ? 'one shared branch' : 'a branch per thread'}`])];
    if (str(p.justification)) out.push(field_('Why this way', str(p.justification), { fold: true }));
    const dirty = Number(p.main_uncommitted) || 0;
    if (dirty) out.push(el('p', 'rc-item-warn', `${dirty} uncommitted ${dirty === 1 ? 'change' : 'changes'} in the main checkout.`));
    const threads = Array.isArray(p.threads) ? p.threads : [];
    out.push(el('h3', 'rc-item-h', threads.length === 1 ? '1 thread' : `${threads.length} threads`));
    for (const t of threads) {
      const card = el('div', 'rc-thread');
      const top = el('div', 'rc-thread-top');
      top.append(dataEl('span', 'rc-thread-name', str(t.name), true));
      const meta = [str(t.model), str(t.repo) && str(t.repo) !== '.' ? str(t.repo) : '', str(t.ticket)].filter(Boolean).join(' · ');
      if (meta) top.append(dataEl('span', 'rc-thread-meta', meta, true));
      card.append(top);
      if (str(t.scope)) card.append(field_('Scope', str(t.scope), { fold: true }));
      if (str(t.goal)) card.append(field_('Goal', str(t.goal), { fold: true }));
      const pick = modelPick(t);
      if (pick) card.append(pick);
      out.push(card);
    }
    return out;
  }
  // A thread's model, chosen before approving: the proposed one first and
  // selected, then the configured list. Only the model of a thread changes.
  function modelPick(t) {
    const list = st.item?.models || [];
    const name = str(t.name);
    if (!list.length || !name) return null;
    const proposed = str(t.model);
    const box = el('label', 'rc-kv rc-model-pick');
    box.append(el('span', 'rc-kv-k', 'Model'));
    const sel = el('select', 'rc-select rc-model-select');
    sel.dataset.thread = name;
    const opt = (value, label) => { const o = el('option', null, label); o.value = value; return o; };
    if (!list.some(m => m.id === proposed)) sel.append(opt(proposed, proposed ? `${proposed} (proposed)` : 'Default (proposed)'));
    for (const m of list) sel.append(opt(m.id, m.id === proposed ? `${m.label} (proposed)` : m.label));
    sel.value = proposed;
    sel.addEventListener('change', () => { if (st.item) st.item.picked[name] = sel.value; });
    box.append(sel);
    return box;
  }
  // A tip is shown as 7 hex characters; anything else is data, shown marked.
  const short = h => (/^[0-9a-f]{7,64}$/.test(str(h)) ? str(h).slice(0, 7) : str(h));
  function mergeView(m) {
    const out = [head(`Merge of ${str(m.sandbox)}`, [str(m.variant) === 'shared' ? 'one shared branch' : 'a branch per thread'])];
    const order = Array.isArray(m.order) ? m.order.map(str) : [];
    if (order.length) {
      const ol = el('ol', 'rc-order');
      for (const o of order) ol.append(dataEl('li', null, o, true));
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
        li.append(el('span', 'rc-dist-k', d?.kind === 'warning' ? 'Warning' : 'Decision'), dataEl('span', null, str(d?.text)));
        ul.append(li);
      }
      const box = el('div', 'rc-kv');
      box.append(el('p', 'rc-kv-k', 'Kept for later sessions'), ul);
      out.push(box);
    }
    for (const b of Array.isArray(m.branches) ? m.branches : []) {
      const card = el('div', 'rc-branch');
      const top = el('div', 'rc-thread-top');
      top.append(dataEl('span', 'rc-thread-name', str(b.branch) || str(b.thread), true));
      top.append(b.tip ? dataEl('span', 'rc-thread-meta', short(b.tip), true) : el('span', 'rc-thread-meta', 'unreadable'));
      card.append(top);
      const base = b.base && typeof b.base === 'object' ? b.base : {};
      const ins = Number(b.insertions) || 0, del = Number(b.deletions) || 0;
      card.append(partsInto(el('p', 'rc-branch-line'), ['into ', { data: str(base.branch) }, ' at ', base.tip ? { data: short(base.tip) } : 'unreadable', ...(str(b.repo) && str(b.repo) !== '.' ? [' (', { data: str(b.repo) }, ')'] : [])]));
      const stat = el('p', 'rc-branch-stat');
      stat.append(el('span', null, `${Number(b.ahead) || 0} ${Number(b.ahead) === 1 ? 'commit' : 'commits'} · ${Number(b.files_changed) || 0} files `), el('span', 'rc-ins', `+${ins}`), ' ', el('span', 'rc-del', `−${del}`));
      card.append(stat);
      if (b.known_conflict === true) card.append(el('p', 'rc-item-warn', 'An earlier attempt stopped at a conflict on this branch.'));
      const files = Array.isArray(b.files) ? b.files : [];
      if (files.length) {
        const ul = el('ul', 'rc-files');
        for (const f of files) {
          const li = el('li');
          li.append(dataEl('span', 'rc-file-path', str(f?.path), true));
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
    const out = [head(str(r.host), ['asked by ', { data: str(r.sandbox) }])];
    if (str(r.detail)) out.push(field_('What sbx says', str(r.detail), { fold: true, mono: true }));
    if (str(r.created_at)) out.push(field_('Asked at', str(r.created_at)));
    const opts = Array.isArray(r.options) ? r.options.filter(o => o && typeof o.id === 'string') : [];
    if (!opts.length) out.push(el('p', 'rc-item-note', 'Answer this on your Mac.'));
    // What each option sbx offers does, in the daemon's words.
    for (const o of opts) {
      const box = el('div', 'rc-kv rc-net-opt');
      box.append(dataEl('p', 'rc-kv-k', str(o.label) || o.id));
      if (str(o.effect)) box.append(dataEl('p', 'rc-kv-v', str(o.effect)));
      // sbx 0.51's Allow writes a lasting rule (checked); an id sbxw does not know may.
      if (o.lasting === true) box.append(el('p', 'rc-item-warn', o.id === 'allow' ? 'Adds a lasting network rule.' : 'May leave a lasting network rule.'));
      out.push(box);
    }
    return out;
  }
  function stoppedView(sb, confirm) {
    const sub = [str(sb.agent) || (sb.chat ? 'chat' : ''), str(sb.created_at)].filter(Boolean).flatMap((x, i) => (i ? [' · ', { data: x }] : [{ data: x }]));
    const out = [head(str(sb.name), sub)];
    const lines = Array.isArray(confirm) ? confirm.map(str).filter(Boolean) : [];
    const box = el('div', 'rc-confirm-lines');
    // The daemon's own sentences, verbatim, as text.
    for (const l of lines) box.append(dataEl('p', null, l));
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
      const b = el('button', `rc-btn ${cls}`);
      if (option) b.append(dataEl('span', null, label)); else b.textContent = label;
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
    // A request with no option to answer: say so.
    if (what === 'network' && !list.length) list.push(el('p', 'rc-item-note', 'Only the Mac can answer this request.'));
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
    // A plan approval carries the models picked in the detail (only those
    // that differ from the proposed ones), bound to the challenge.
    const models = kind === 'plan_approve' ? rcModelChanges(st.item.data.threads, st.item.picked, st.item.models) : {};
    sheet = { kind, option: option || null, models, ctl: null, token: ++sheetSeq };
    const title = $('sheet-title');
    title.replaceChildren();
    if (kind === 'network_respond') partsInto(title, ['Answer ', { data: option?.label ?? option?.id ?? '' }, '?']);
    else title.textContent = SHEET_TITLE[kind];
    // What will happen, to what: the names isolated, in monospace, marked.
    const line = $('sheet-line');
    line.replaceChildren();
    partsInto(line, rcActParts(kind, st.item.data, option, models));
    // A network answer: what the option does, and a lasting rule flagged.
    const more = $('sheet-more');
    more.replaceChildren();
    if (kind === 'network_respond') {
      const o = (Array.isArray(st.item.data.options) ? st.item.data.options : []).find(x => x && x.id === option?.id);
      const sb = { data: str(st.item.data.sandbox) };
      if (o && o.id === 'allow') more.append(partsInto(el('p', 'rc-item-warn'), [{ data: str(o.label) || o.id }, ' ', { data: str(st.item.data.host) }, ': adds a lasting network rule to ', sb, ' (remove it on the Mac with sbx policy rm).']));
      else if (o && o.lasting === true) more.append(partsInto(el('p', 'rc-item-warn'), ['Lasting: ', { data: str(o.label) || o.id }, ' may add a network rule to ', sb, ' that stays until you remove it on your Mac.']));
      if (o && str(o.effect)) more.append(dataEl('p', 'rc-sheet-effect', str(o.effect)));
    }
    more.hidden = !more.childNodes.length;
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
    const ch = await l2.challenge(s.kind, target, item.digest, s.option?.id, s.models);
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
    const res = await l2.act(s.kind, target, assertion, s.option?.id, s.models);
    if (!live()) return;
    if (!res.ok) return failed(res, s);
    closeSheet();
    showApprovals(rcActDone(s.kind, res.data));
  });
  function failed(res, s) {
    if (res.data?.reason === 'changed') {
      // Read it again: the detail is fetched anew, the sheet closes. A
      // merge says which branch moved and what was already merged (v1.2):
      // its sentence is shown as it is, as data.
      const it = st.item;
      const why = s.kind === 'merge' && typeof res.data.error === 'string' && res.data.error.trim() ? res.data.error.trim().slice(0, 600) : '';
      closeSheet();
      if (it) openItem(it.what, it.id, why ? [`${RC_L2_REASON.changed} `, { data: why }] : RC_L2_REASON.changed);
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
  let pkPollNow = null; // the running ceremony's poll, to resume it
  // The ceremony's request_id (v1.1): in this variable only, never stored
  // or put in a URL. It survives the page being hidden (a phone that locks
  // while the person types the digits at the Mac); it goes when the
  // ceremony ends, the Face ID screen is left, or the page is unloaded.
  let pkReq = null;
  function stopPasskeyPoll() { if (pkPoll) { clearTimeout(pkPoll); pkPoll = null; } pkReq = null; pkPollNow = null; }
  document.addEventListener('visibilitychange', () => {
    if (!pkReq || !pkPollNow) return;
    if (document.visibilityState === 'hidden') {
      // Hidden: no polling, the id kept.
      if (pkPoll) { clearTimeout(pkPoll); pkPoll = null; }
    } else if (!pkPoll && st.screen === 's-passkey') {
      // Back: look once now with the same id, and carry on.
      pkPollNow();
    }
  });
  window.addEventListener('pagehide', stopPasskeyPoll);
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
    pkReq = /^[0-9a-f]{32}$/.test(res.data.request_id) ? res.data.request_id : null;
    if (!pkReq) return pkShow('failed');
    pkShow('pending', String(res.data.check || ''));
    const until = Date.now() + 6 * 60 * 1000;
    const poll = async () => {
      pkPoll = null;
      if (st.screen !== 's-passkey' || !pkReq) return;
      if (document.visibilityState === 'hidden') return; // resumed when visible
      if (Date.now() > until) { stopPasskeyPoll(); return pkShow('expired'); }
      const r = await l2.passkeyStatus(pkReq);
      if (st.screen !== 's-passkey' || !pkReq) return;
      const state = r.ok ? r.data?.state : null;
      if (state === 'granted') return pkShow('granted');
      if (state === 'refused') { stopPasskeyPoll(); return pkShow('refused'); }
      // "none": this request is not the device's any more (expired, or a
      // newer one replaced it): start again, never guess.
      if (state === 'none') { stopPasskeyPoll(); return pkShow('expired'); }
      if (document.visibilityState !== 'hidden') pkPoll = setTimeout(poll, 2000);
    };
    pkPollNow = poll;
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
    // Lost the request (a reload, the page hidden): start again.
    const rid = pkReq;
    if (!rid) return pkShow('expired');
    const o = await l2.passkeyOptions(rid);
    if (st.screen !== 's-passkey') return;
    if (!o.ok) { pkShow(o.data?.reason === 'no_grant' ? 'expired' : 'failed'); if (o.data?.reason) $('pk-text').textContent = rcL2Fail(o); return; }
    const publicKey = rcCreateOptions(o.data?.public_key);
    if (!publicKey) { pkShow('failed'); $('pk-text').textContent = 'The Mac sent options this page cannot use. Nothing was saved.'; return; }
    let cred;
    try {
      cred = await navigator.credentials.create({ publicKey });
    } catch (e) {
      stopPasskeyPoll();
      if (st.screen === 's-passkey') pkShow(rcCeremonyError(e));
      return;
    }
    if (st.screen !== 's-passkey') return;
    const body = rcRegistrationBody(cred);
    if (!body) { pkShow('failed'); $('pk-text').textContent = 'The browser gave an answer this page cannot read. Nothing was saved.'; return; }
    const r = await l2.passkeyRegister(rid, body);
    // The ceremony is over, whatever the answer: the id goes.
    stopPasskeyPoll();
    if (st.screen !== 's-passkey') return;
    // no_grant: the grant expired or was used (v1.0); passkey: a check failed.
    if (!r.ok) return pkShow(r.data?.reason === 'passkey' ? 'rejected' : r.data?.reason === 'no_grant' ? 'expired' : 'failed');
    // Whether Face ID is set up is GET /r/approvals' to say (v1.1).
    await refreshL2();
    if (st.screen !== 's-passkey') return;
    pkShow(st.l2?.passkey === true ? 'registered' : 'failed');
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
