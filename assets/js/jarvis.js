// ── Jarvis's proposals ───────────────────────────────────────────────────
// Jarvis (a Claude session in the daemon-owned `sbxw-jarvis` sandbox) can
// propose changes to sbxw's configuration: kits, skills, settings, network,
// MCP servers. The daemon holds each proposal, validates it, and applies it
// only once the person approves it, here. See docs/jarvis.md.
//
// * **The diff is the sandbox's word.** Summary, titles and diffs are put on
//   the page with textContent only, never as HTML.
// * **It never takes the keyboard.** The dialog opens beside the page, not
//   as a modal, and gives the focus back to whatever had it (a terminal):
//   click into it to use it.
// * **Voice approval depends on the risk.** A low-risk proposal is read and
//   a spoken yes approves it (`via: "voice"`); a high-risk one is read and
//   must be approved on screen: a spoken yes only says so again, and so does
//   the daemon's 409 if a voice approval reaches it anyway.
//
// Loaded after concierge.js: it uses its voice (vcSpeak, vcArm, vcLang…)
// through `typeof` checks, so it still works with the voice not set up.
// Listens on `sbxwStream` for `jarvis` events, as plans.js and relay.js do.

const jvDlg = document.getElementById('jv-dialog');
const jvBody = document.getElementById('jv-body');
const jvFoot = document.getElementById('jv-foot');
const jvRiskEl = document.getElementById('jv-risk');
const jvQueueEl = document.getElementById('jv-queue');
const jvBadge = document.getElementById('jv-badge');

const jvProposals = new Map(); // id → proposal, as the daemon last sent it
let jvCurrentId = null;
let jvBusy = false;           // an answer is on its way to the daemon
let jvSeeded = false;         // the first list is in: what arrives later is new
const jvAcked = new Set();    // proposals whose host command the person ticked as read
let jvError = null;           // { id, text }: the last refused answer, shown in the dialog

// A change that runs a command on this machine, outside any sandbox (a stdio
// MCP server). The daemon flags the proposal `needs_ack` and refuses an
// approval without `ack_host_command: true`, which only the checkbox sends.
function jvHostCommands(p) { return (p.changes || []).map(c => c.host_command).filter(Boolean); }
function jvNeedsAck(p) { return !!p.needs_ack || jvHostCommands(p).length > 0; }
// On screen only: high risk, or a host command to acknowledge.
function jvScreenOnly(p) { return p.risk === 'high' || jvNeedsAck(p); }

function jvPending() {
  return [...jvProposals.values()].filter(p => p.state === 'pending')
    .sort((a, b) => (Number(a.created) || 0) - (Number(b.created) || 0));
}
function jvCurrent() { return jvCurrentId ? jvProposals.get(jvCurrentId) : null; }

function jvVoice() { return typeof vcSpeak === 'function' && typeof vcSpeech !== 'undefined'; }
function jvLang(fr, en) { return typeof vcLang === 'function' ? vcLang(fr, en) : en; }
// Only with the voice installed and on: a muted or absent voice says nothing.
function jvSpeak(text) { if (jvVoice() && !vcSpeech.muted && vcCanSpeak()) vcSpeak(text); }
function jvSay(who, text) { if (typeof vcSay === 'function') vcSay(who, text); }

// The daemon's change kinds, as the person reads them.
const JV_KINDS = {
  kit_write: 'kit', kit_remove: 'kit removed', kits_line: 'kits line', setting: 'setting', voice_word: 'pronunciation',
  network_allow: 'network', mcp_server: 'MCP server', mcp_remove: 'MCP server removed',
};

const JV_HIGH = () => jvLang(
  "Celle-ci est à risque élevé : lis-la et approuve-la à l'écran.",
  'This one is high risk: it must be read and approved on screen.');

// ── Rendering ────────────────────────────────────────────────────────────
function jvEl(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

// What happened to one change once the proposal is answered: the daemon's
// `results[i]` ({ok, message, restart}), one per change, in order. Without
// it, the proposal's own state stands for every change.
function jvChangeResult(p, c, i) {
  if (p.state === 'pending') return null;
  const r = (p.results || [])[i];
  if (r) {
    const text = r.ok ? (r.restart ? 'Applied, takes effect when the daemon restarts' : 'Applied') : 'Failed';
    return { ok: !!r.ok, text, message: r.message || '' };
  }
  if (p.state === 'applied') return { ok: true, text: 'Applied' };
  if (p.state === 'refused') return { ok: null, text: 'Not applied (refused)' };
  if (p.state === 'failed') return { ok: false, text: 'Not applied' };
  return null;
}

function jvRender() {
  const p = jvCurrent();
  jvRenderBadge();
  if (!p) { jvBody.replaceChildren(); jvFoot.replaceChildren(); return; }
  const high = jvScreenOnly(p);
  jvRiskEl.textContent = high ? 'High risk' : 'Low risk';
  jvRiskEl.className = `jv-risk ${high ? 'jv-risk-high' : 'jv-risk-low'}`;
  const waiting = jvPending().filter(x => x.id !== p.id).length;
  jvQueueEl.hidden = !waiting;
  jvQueueEl.textContent = waiting ? `${waiting} more waiting` : '';

  const parts = [jvEl('p', 'jv-summary', p.summary || '(no summary)')];
  if ((high || jvNeedsAck(p)) && p.state === 'pending') {
    parts.push(jvEl('p', 'jv-note', 'High risk: read every change below before you approve. A spoken yes does not approve this one.'));
  }
  const list = jvEl('ol', 'jv-changes');
  (p.changes || []).forEach((c, i) => {
    const li = jvEl('li', 'jv-change');
    const head = jvEl('div', 'jv-change-head');
    head.append(jvEl('span', 'jv-kind', JV_KINDS[c.kind] || c.kind || 'change'), jvEl('span', 'jv-change-title', c.title || ''));
    if (c.risk === 'high' && p.state === 'pending') head.append(jvEl('span', 'jv-result bad', 'high risk'));
    const res = jvChangeResult(p, c, i);
    if (res) head.append(jvEl('span', `jv-result ${res.ok === true ? 'ok' : res.ok === false ? 'bad' : 'dim'}`, res.text));
    li.append(head);
    if (res?.message) li.append(jvEl('p', `jv-result-msg ${res.ok ? '' : 'bad'}`, res.message));
    if (c.host_command) {
      // The full command, as text: the one change that runs code on the host.
      const warn = jvEl('div', 'jv-host');
      warn.setAttribute('role', 'alert');
      warn.append(jvEl('p', 'jv-host-title', 'This command will run on this machine, outside any sandbox:'));
      const cmd = jvEl('pre', 'jv-host-cmd', String(c.host_command));
      cmd.tabIndex = 0;
      cmd.setAttribute('aria-label', 'The command that will run on this machine');
      warn.append(cmd);
      li.append(warn);
    }
    if (c.diff) {
      // The sandbox's text, shown as text: no markup survives.
      const pre = jvEl('pre', 'jv-diff');
      for (const line of String(c.diff).split('\n')) {
        const span = jvEl('span', line.startsWith('+') && !line.startsWith('+++') ? 'jv-add'
          : line.startsWith('-') && !line.startsWith('---') ? 'jv-del' : '', `${line}\n`);
        pre.append(span);
      }
      pre.tabIndex = 0; // scrollable from the keyboard once clicked into
      pre.setAttribute('aria-label', `Diff of ${c.title || 'this change'}`);
      li.append(pre);
    }
    list.append(li);
  });
  if (!(p.changes || []).length) list.append(jvEl('li', 'jv-dim', 'No change listed.'));
  parts.push(list);
  if (jvError?.id === p.id && p.state === 'pending') parts.push(jvEl('p', 'jv-outcome jv-failed', jvError.text));
  if (p.state !== 'pending') {
    const outcome = p.state === 'applied' ? 'Applied.' : p.state === 'refused' ? 'Refused: nothing was changed.' : 'Failed.';
    const o = jvEl('p', `jv-outcome jv-${p.state}`, outcome);
    if (p.error) o.append(jvEl('span', 'jv-error', ` ${p.error}`));
    parts.push(o);
  }
  jvBody.replaceChildren(...parts);

  jvFoot.replaceChildren();
  if (p.state === 'pending') {
    const reason = jvEl('input', 'jv-reason');
    reason.type = 'text';
    reason.placeholder = 'Why not (optional, told to Jarvis)';
    reason.setAttribute('aria-label', 'Reason for refusing');
    const refuse = jvEl('button', 'jv-btn', 'Refuse');
    refuse.type = 'button';
    refuse.dataset.jvAction = 'refuse';
    refuse.disabled = jvBusy;
    refuse.onclick = () => jvRefuse(p.id, reason.value.trim());
    const approve = jvEl('button', 'btn-create', 'Approve');
    approve.type = 'button';
    approve.dataset.jvAction = 'approve';
    const ackNeeded = jvNeedsAck(p);
    approve.disabled = jvBusy || (ackNeeded && !jvAcked.has(p.id));
    approve.onclick = () => jvApprove(p.id, 'click');
    if (ackNeeded) {
      const label = jvEl('label', 'jv-ack');
      const box = jvEl('input');
      box.type = 'checkbox';
      box.dataset.jvAction = 'ack';
      box.checked = jvAcked.has(p.id);
      box.onchange = () => {
        if (box.checked) jvAcked.add(p.id); else jvAcked.delete(p.id);
        approve.disabled = jvBusy || !box.checked;
      };
      label.append(box, document.createTextNode(' I have read this command'));
      jvFoot.append(label);
    }
    jvFoot.append(reason, refuse, approve);
  } else {
    const next = jvPending()[0];
    const btn = jvEl('button', 'jv-btn', next ? 'Next proposal' : 'Close');
    btn.type = 'button';
    btn.onclick = () => (next ? jvShow(next.id) : jvClose());
    jvFoot.append(btn);
  }
}

function jvRenderBadge() {
  const n = jvPending().length;
  jvBadge.hidden = !n;
  jvBadge.textContent = n ? `Jarvis · ${n} to review` : '';
  jvBadge.title = n ? `Jarvis proposes ${n === 1 ? 'a change' : `${n} changes`} to sbxw's configuration: click to review` : '';
}

// ── Open / close ─────────────────────────────────────────────────────────
// Not modal: the page stays usable and the focus goes back where it was,
// so a terminal being typed into keeps its keystrokes.
function jvShow(id) {
  if (id) jvCurrentId = id;
  if (!jvCurrentId) jvCurrentId = jvPending()[0]?.id || null;
  if (!jvCurrentId) return;
  const had = document.activeElement;
  if (!jvDlg.open) jvDlg.show();
  jvRender();
  if (had && had !== document.body && had.isConnected && !jvDlg.contains(had)) had.focus({ preventScroll: true });
  else jvDlg.querySelector('.modal-close')?.blur();
}
function jvClose() {
  jvDlg.close();
  jvCurrentId = null;
  if (typeof vcPending !== 'undefined' && vcPending?.kind === 'jarvis' && typeof vcDisarm === 'function') vcDisarm();
  jvRenderBadge();
}
document.getElementById('jv-close').addEventListener('click', jvClose);
jvDlg.addEventListener('keydown', e => { if (e.key === 'Escape') { e.preventDefault(); jvClose(); } });
jvDlg.addEventListener('cancel', e => { e.preventDefault(); jvClose(); });
jvBadge.addEventListener('click', () => jvShow(jvPending()[0]?.id));

// ── Answers ──────────────────────────────────────────────────────────────
// The answer's status matters: 409 on a voice approval means "high risk,
// on screen only", which is said rather than shown as a failure.
async function jvPost(url, body) {
  try {
    const r = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    let data = {};
    try { data = await r.json(); } catch (_) {}
    return { status: r.status, ok: r.ok && data.ok !== false, data };
  } catch (e) {
    return { status: 0, ok: false, data: { error: e.message } };
  }
}

// The daemon may answer with the proposal as it now stands; else it comes
// on the `jarvis` event, or from a fresh list.
function jvTake(data) {
  const p = data?.proposal || (data?.id && data?.state ? data : null);
  if (p) jvIngest(p);
  else jvLoad();
}

// Not answerable (gone, answered, or one already on its way): a yes said or
// typed in the panel gets a reason there; a click sees the dialog.
function jvNotNow(p, via) {
  if (via === 'voice') {
    jvSay('err', !p || p.state !== 'pending' ? "Jarvis's proposal is no longer waiting for an answer."
      : 'Wait: an answer to a Jarvis proposal is already on its way.');
  }
  return false;
}

async function jvApprove(id, via) {
  const p = jvProposals.get(id);
  if (!p || p.state !== 'pending' || jvBusy) return jvNotNow(p, via);
  // A spoken yes never approves a high-risk proposal: say so, again.
  if (via === 'voice' && jvScreenOnly(p)) return jvHighRisk(p);
  const body = { via };
  // Only the ticked checkbox acknowledges a host command; never the voice.
  if (via === 'click' && jvNeedsAck(p)) {
    if (!jvAcked.has(id)) { jvError = { id, text: 'Tick "I have read this command" first.' }; jvRender(); return; }
    body.ack_host_command = true;
  }
  jvBusy = true; jvError = null; jvRender();
  const r = await jvPost(`/api/jarvis/proposals/${encodeURIComponent(id)}/approve`, body);
  jvBusy = false;
  if (r.status === 409 && via === 'voice') { jvRender(); return jvHighRisk(p); }
  if (!r.ok) {
    const err = r.data?.error || `HTTP ${r.status}`;
    // Shown in the dialog, beside the buttons: a 409 is an answer, not a silence.
    jvError = { id, text: `Not applied: ${err}` };
    showToast(`Jarvis's proposal was not applied: ${err}`, 'error');
    jvSay('err', `Proposal not applied: ${err}`);
    jvRender();
    return;
  }
  jvAcked.delete(id);
  jvTake(r.data);
  jvSay('dim', `Approved: ${p.summary}`);
}

async function jvRefuse(id, reason, via) {
  const p = jvProposals.get(id);
  if (!p || p.state !== 'pending' || jvBusy) return jvNotNow(p, via);
  jvBusy = true; jvRender();
  const r = await jvPost(`/api/jarvis/proposals/${encodeURIComponent(id)}/refuse`, reason ? { reason } : {});
  jvBusy = false;
  if (!r.ok) {
    const err = r.data?.error || `HTTP ${r.status}`;
    showToast(`Could not refuse Jarvis's proposal: ${err}`, 'error');
    if (via === 'voice') jvSay('err', `Proposal not refused: ${err}`);
    jvRender();
    return false;
  }
  jvTake(r.data);
  jvSay('dim', `Refused: ${p.summary}`);
}

// The high-risk answer to a spoken yes, shown and said: never silent.
function jvHighRisk(p) {
  jvShow(p.id);
  jvSay('dim', 'High risk: approve it on screen, with the Approve button.');
  jvSpeak(JV_HIGH());
  jvArm(p);
}

// ── Voice ────────────────────────────────────────────────────────────────
// A new pending proposal is read; its summary only (the diff is for the
// eyes). The yes or no that follows reaches it through concierge.js's
// `vcHear`, which hands both to the `jarvis` kind armed here.
function jvArm(p) {
  if (typeof vcArm !== 'function') return;
  vcArm({
    kind: 'jarvis', id: p.id,
    yes: () => jvApprove(p.id, 'voice'),
    no: async () => { if ((await jvRefuse(p.id, '', 'voice')) !== false) jvSpeak(jvLang('Refusé.', 'Refused.')); },
  });
}
function jvAnnounce(p) {
  const intro = jvLang(`Jarvis propose : ${p.summary}.`, `Jarvis proposes: ${p.summary}.`);
  if (jvScreenOnly(p)) {
    jvSpeak(`${intro} ${JV_HIGH()}`);
  } else {
    jvSpeak(`${intro} ${jvLang("Je l'applique ?", 'Shall I apply it?')}`);
  }
  jvArm(p);
}

// ── Ingest ───────────────────────────────────────────────────────────────
function jvIngest(p, seed = false) {
  if (!p || !p.id) return;
  const prev = jvProposals.get(p.id);
  jvProposals.set(p.id, p);
  const isNew = !prev && p.state === 'pending';
  if (prev?.state === 'pending' && p.state !== 'pending') {
    // Answered (here, by voice, or in another tab): the dialog shows the
    // result per change; the voice says how it went.
    if (typeof vcPending !== 'undefined' && vcPending?.kind === 'jarvis' && vcPending.id === p.id) vcDisarm();
    if (p.state === 'applied') jvSpeak(jvLang('Appliqué.', 'Applied.'));
    if (p.state === 'failed') {
      jvSpeak(jvLang("Ça n'a pas marché.", 'It failed.'));
      showToast(`Jarvis's proposal failed: ${p.error || 'see the dialog'}`, 'error');
    }
  }
  if (isNew) {
    if (!jvDlg.open) jvShow(p.id);
    if (!seed) jvAnnounce(p);
  }
  jvRender();
}

function jvIngestAll(list, seed = false) {
  const ids = new Set(list.map(p => p.id));
  for (const id of [...jvProposals.keys()]) if (!ids.has(id) && id !== jvCurrentId) jvProposals.delete(id);
  list.forEach(p => jvIngest(p, seed));
}

async function jvLoad() {
  try {
    const r = await fetch('/api/jarvis');
    if (!r.ok) return;
    const d = await r.json();
    jvIngestAll(d.proposals || [], !jvSeeded);
    jvSeeded = true;
  } catch (_) {}
}
jvLoad();

// The event may carry the whole list, one proposal, or nothing to say what
// changed: each is taken as it comes.
sbxwStream.addEventListener('jarvis', ev => {
  let d = null;
  try { d = JSON.parse(ev.data); } catch (_) {}
  if (Array.isArray(d?.proposals)) jvIngestAll(d.proposals);
  else if (d?.id) jvIngest(d);
  else jvLoad();
});
