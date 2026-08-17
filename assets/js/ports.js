// ── Ports modal ───────────────────────────────────────────────────────────
const portsOverlay  = document.getElementById('ports-modal-overlay');
const portsNameEl   = document.getElementById('ports-modal-name');
const portsTbody    = document.getElementById('ports-modal-tbody');
const hostsTbody    = document.getElementById('hosts-modal-tbody');
const policyContent = document.getElementById('policy-modal-content');
const addSbxPort    = document.getElementById('add-sbx-port');
const addHostPort   = document.getElementById('add-host-port');
const addHostIp     = document.getElementById('add-host-ip');
const addAlias      = document.getElementById('add-alias');
const btnAddMapping = document.getElementById('btn-add-mapping');
const portsAddError = document.getElementById('ports-add-error');
let portsTarget = null;

function renderPortsTable(ports) {
  if (!ports.length) {
    portsTbody.innerHTML = '<tr><td colspan="4" class="ports-empty-msg">No ports published</td></tr>';
    return;
  }
  portsTbody.innerHTML = ports.map(p =>
    `<tr>
      <td><strong>${p.sandbox_port}</strong></td>
      <td style="font-family:ui-monospace,monospace;font-size:11px;color:#8b949e">${p.host_ip}:${p.host_port}</td>
      <td><span class="proto-badge">${p.proto}</span></td>
      <td><button class="btn-del-port" data-spec="${p.spec}" title="Unpublish">✕</button></td>
    </tr>`
  ).join('');
  portsTbody.querySelectorAll('.btn-del-port').forEach(btn => {
    btn.addEventListener('click', () => deletePort(btn.dataset.spec, btn));
  });
}

function renderHostsTable(hostEntries) {
  const rows = hostEntries || [];
  if (!rows.length) {
    hostsTbody.innerHTML = '<tr><td colspan="2" class="ports-empty-msg">No /etc/hosts aliases</td></tr>';
    return;
  }
  hostsTbody.innerHTML = rows.map(e =>
    `<tr>
      <td style="color:#c9d1d9">${e.hostname}</td>
      <td style="font-family:ui-monospace,monospace;font-size:11px;color:#8b949e">${e.ip}</td>
    </tr>`
  ).join('');
}

// Policy text comes from sbx (and, for a governance denial, from whatever an
// organisation configured), so it is escaped rather than interpolated raw —
// unlike the port/hostname values above, which sbxw itself constrains.
// `escHtml` lives in util.js, which every other panel needing it also loads.

// `allow`/`deny` get a colour wherever they appear — matching on the value, not
// on a column name, so a renamed or reordered `sbx policy ls` column still
// reads correctly.
function policyCell(cell) {
  const v = String(cell || '').trim();
  const kind = v.toLowerCase();
  if (kind === 'allow' || kind === 'deny')
    return `<span class="policy-badge ${kind}">${escHtml(v)}</span>`;
  return `<span class="policy-cell">${escHtml(v)}</span>`;
}

// A policy id is a UUID; only its first block is worth the width, the rest goes
// in the tooltip. A named policy ("local-policy") is short already — keep it.
function policyShortId(id) {
  const v = String(id || '').trim();
  return /^[0-9a-f-]{20,}$/i.test(v) ? v.split('-')[0] : v;
}

// "sandbox:sbxw-2" is noise once the list is already filtered to this sandbox;
// what matters is whether a policy is specific to it or applies everywhere.
function policyScopeLabel(scope) {
  const v = String(scope || '').trim();
  if (!v || v.toLowerCase() === 'all') return 'all sandboxes';
  if (v.toLowerCase().startsWith('sandbox:')) return 'this sandbox';
  return v;
}

// Split a SUMMARY — "network: 159 allow; filesystem read: 1 allow" — into one
// chip per clause, with the count pulled out so the number reads first.
function policyRuleChips(summary) {
  const clauses = String(summary || '').split(';').map(s => s.trim()).filter(Boolean);
  if (!clauses.length) return '';
  return `<div class="policy-chips">` + clauses.map(clause => {
    const m = clause.match(/^(.*?):\s*(\d+)\s*(\w+)$/);
    const deny = /deny/i.test(clause) ? ' deny' : '';
    if (!m) return `<span class="policy-rule${deny}">${escHtml(clause)}</span>`;
    return `<span class="policy-rule${deny}"><b>${escHtml(m[2])}</b> ${escHtml(m[1])} ${escHtml(m[3])}</span>`;
  }).join('') + `</div>`;
}

// One card per policy document, laid out by column *role* so a reordered or
// renamed sbx column doesn't misplace anything. Falls back to the generic table
// when the roles aren't recognised at all.
function renderPolicyEntries(data) {
  const roles = data.roles || [];
  const col = role => roles.indexOf(role);
  const iId = col('id'), iSrc = col('source'), iScope = col('applies'), iSum = col('summary');
  if (iSrc < 0 && iScope < 0 && iSum < 0) return null;

  const at = (row, i) => (i >= 0 ? String(row[i] || '').trim() : '');
  // Sandbox-specific policies first: they are the ones about *this* sandbox.
  const rows = data.rows.slice().sort((a, b) => {
    const g = r => (policyScopeLabel(at(r, iScope)) === 'all sandboxes' ? 1 : 0);
    return g(a) - g(b);
  });

  return rows.map(row => {
    const src = at(row, iSrc);
    const known = ['local', 'kit', 'corporate'].includes(src.toLowerCase());
    const scope = policyScopeLabel(at(row, iScope));
    const id = at(row, iId);
    // Columns we have no role for still carry information — append them rather
    // than drop them.
    const extra = row
      .map((c, i) => ([iId, iSrc, iScope, iSum].includes(i) ? '' : String(c || '').trim()))
      .filter(Boolean)
      .map(c => `<span class="policy-rule">${escHtml(c)}</span>`)
      .join('');
    return (
      `<div class="policy-entry${scope === 'all sandboxes' ? ' global' : ''}">` +
        `<div class="policy-entry-head">` +
          (src ? `<span class="policy-badge src-${known ? src.toLowerCase() : 'other'}">${escHtml(src)}</span>` : '') +
          `<span class="policy-scope">${escHtml(scope)}</span>` +
          (id ? `<span class="policy-id" title="${escHtml(id)}">${escHtml(policyShortId(id))}</span>` : '') +
        `</div>` +
        policyRuleChips(at(row, iSum)) +
        (extra ? `<div class="policy-chips">${extra}</div>` : '') +
      `</div>`
    );
  }).join('');
}

// `actions` is an optional array of trailing-cell HTML, one per row, aligned by
// index — the per-row delete buttons.
function policyTableHtml(columns, rows, actions) {
  const withActions = Array.isArray(actions);
  return `<table class="ports-table"><thead><tr>` +
    columns.map(c => `<th>${escHtml(c)}</th>`).join('') +
    (withActions ? '<th></th>' : '') +
    `</tr></thead><tbody>` +
    rows.map((r, i) =>
      `<tr>${r.map(c => `<td>${policyCell(c)}</td>`).join('')}` +
      (withActions ? `<td>${actions[i] || ''}</td>` : '') +
      `</tr>`).join('') +
    `</tbody></table>`;
}

// A rule sbxw must not offer to delete: `org` rules are governance, and sbx
// refuses to remove them anyway — an enabled button that always fails is worse
// than none. Everything else is the user's own local or kit policy.
function policyRuleIsGoverned(source) {
  return ['org', 'corporate', 'system'].includes(String(source || '').trim().toLowerCase());
}

function policySectionLabel(text, hint, countId, count) {
  return `<div class="ports-section-label" style="margin-top:.7rem">${text}` +
    (hint ? ` <span style="color:#6e7681;font-weight:400">(${escHtml(hint)})</span>` : '') +
    (countId ? ` <span class="policy-count" id="${countId}">${count}</span>` : '') +
    `</div>`;
}

// Does a view hold a table worth rendering?
function policyHasRows(view) {
  return !!(view && !view.error && view.columns && view.columns.length && view.rows.length);
}

// A column already implied by the section is dead width: the rows are scoped to
// this sandbox, so `SANDBOX` / `APPLIES TO` would repeat the same value on every
// line where the resources and reasons need the room.
function policyDropImpliedColumns(view, roles_to_drop) {
  const roles = view.roles || [];
  const keep = view.columns.map((_, i) => !roles_to_drop.includes(roles[i]));
  return {
    columns: view.columns.filter((_, i) => keep[i]),
    rows: view.rows.map(r => r.filter((_, i) => keep[i])),
  };
}

// The rule-level view (`sbx policy ls <sandbox> --wide`): one row per rule, with
// the resource it covers. This is the section that actually answers "what can
// this sandbox reach?", so it leads — and it gets a filter box, because a global
// policy runs to a couple of hundred rules.
// Map each policy id to the scope its overview row reported. The rule-level
// listing carries a policy id but not always a scope, and "does deleting this
// affect every sandbox?" is exactly what you need to know before confirming.
function policyScopeById(policies) {
  const out = {};
  if (!policyHasRows(policies)) return out;
  const roles = policies.roles || [];
  const iId = roles.indexOf('id'), iScope = roles.indexOf('applies');
  if (iId < 0 || iScope < 0) return out;
  for (const row of policies.rows) {
    const id = String(row[iId] || '').trim();
    if (id) out[id] = policyScopeLabel(row[iScope]);
  }
  return out;
}

function renderPolicyRules(view, policies) {
  if (!policyHasRows(view)) return policyAddForm();
  const roles = view.roles || [];
  const at = (row, role) => {
    const i = roles.indexOf(role);
    return i >= 0 ? String(row[i] || '').trim() : '';
  };
  const scopeById = policyScopeById(policies);

  // Delete needs an id sbx will accept, and it must be a *rule* id: handing a
  // policy id to `policy rm` would take out every rule in that policy.
  const deletable = roles.includes('rule_id');
  const actions = deletable ? view.rows.map(row => {
    const id = at(row, 'rule_id');
    if (!id || id === '-') return '';
    if (policyRuleIsGoverned(at(row, 'source')))
      return `<span class="rule-locked" title="Set by organisation policy — sbx won't let sbxw remove it">🔒</span>`;
    // Own scope column if there is one, else the parent policy's, else nothing
    // — an unknown scope is reported as unknown, never guessed.
    const own = at(row, 'applies') || at(row, 'sandbox');
    const scope = own ? policyScopeLabel(own) : (scopeById[at(row, 'id')] || '');
    return `<button class="btn-del-rule" title="Remove this rule"` +
      ` data-rule="${escHtml(id)}"` +
      ` data-resource="${escHtml(at(row, 'host'))}"` +
      ` data-decision="${escHtml(at(row, 'action'))}"` +
      ` data-scope="${escHtml(scope)}">✕</button>`;
  }) : undefined;

  const { columns, rows } = policyDropImpliedColumns(view, ['applies', 'sandbox']);
  return policySectionLabel('Rules', 'sbx policy ls --wide', 'policy-rule-count', rows.length) +
    `<input id="policy-rule-filter" class="policy-filter" type="text" placeholder="Filter rules — domain, decision, source…" autocomplete="off" spellcheck="false">` +
    `<div class="policy-scroll" id="policy-rules-wrap">${policyTableHtml(columns, rows, actions)}</div>` +
    (view.truncated ? `<div class="policy-note">+ ${view.truncated} more rule${view.truncated === 1 ? '' : 's'} not shown — run <code>sbx policy ls --wide</code> for the full set.</div>` : '') +
    (deletable ? '' : '<div class="policy-note">This sbx\'s listing has no rule-id column, so rules can\'t be removed from here — use <code>sbx policy rm</code>.</div>') +
    policyAddForm();
}

// Always offered, even when the listing failed: adding a rule doesn't depend on
// being able to read the current ones.
function policyAddForm() {
  return `<div class="policy-form">` +
    `<input id="policy-add-res" class="port-input" type="text" autocomplete="off" spellcheck="false"` +
    ` placeholder="example.com, *.acme.dev, host:443" title="Comma-separated resources, in sbx's syntax">` +
    `<select id="policy-add-decision" title="Allow or deny egress to these resources">` +
      `<option value="allow">allow</option><option value="deny">deny</option>` +
    `</select>` +
    `<label class="policy-scope-toggle" title="Write the rule to the host-wide policy instead of this sandbox">` +
      `<input type="checkbox" id="policy-add-global"><span>all sandboxes</span>` +
    `</label>` +
    `<button class="btn-add-mapping" id="policy-add-btn" disabled>＋ Add rule</button>` +
    `<span class="ports-error-msg hidden" id="policy-add-error" style="flex-basis:100%"></span>` +
  `</div>`;
}

// Recent allow/deny decisions: what the rules did, rather than what they say.
// A sbx without `policy log` renders nothing at all rather than an error.
function renderPolicyLog(log) {
  if (!log || log.error) return '';
  if (!log.columns || !log.columns.length)
    return log.raw ? policySectionLabel('Recent egress', 'sbx policy log') + `<pre class="policy-raw">${escHtml(log.raw)}</pre>` : '';
  if (!log.rows.length) return '';
  const { columns, rows } = policyDropImpliedColumns(log, ['sandbox', 'applies']);
  return policySectionLabel('Recent egress', 'sbx policy log') +
    `<div class="policy-scroll">${policyTableHtml(columns, rows)}</div>` +
    (log.truncated ? `<div class="policy-note">+ ${log.truncated} older entr${log.truncated === 1 ? 'y' : 'ies'} — run <code>sbx policy log</code> for the full history.</div>` : '');
}

// The overview: which policies govern this sandbox and where they come from.
// Secondary to the rules themselves, so it sits in a fold.
function renderPolicySources(view) {
  if (!view || view.error) return '';
  if (!view.columns || !view.columns.length)
    return view.raw ? `<details class="policy-details"><summary>Policies governing this sandbox</summary><pre class="policy-raw">${escHtml(view.raw)}</pre></details>` : '';
  if (!view.rows.length) return '';

  const body = renderPolicyEntries(view) ?? policyTableHtml(view.columns, view.rows);
  const hidden = view.other_sandboxes
    ? `<div class="policy-note">${view.other_sandboxes} more polic${view.other_sandboxes === 1 ? 'y' : 'ies'} on this host ${view.other_sandboxes === 1 ? 'is' : 'are'} scoped to other sandboxes.</div>`
    : '';
  return `<details class="policy-details"><summary>Policies governing this sandbox (${view.rows.length})</summary>${body}${hidden}</details>`;
}

function renderPolicy(data) {
  if (!data || (!data.ok && data.error)) {
    const msg = (data && data.error) || 'Error fetching network policy';
    policyContent.innerHTML = `<div class="ports-empty-msg" style="color:#f85149">${escHtml(msg)}</div>`;
    return;
  }

  const parts = [];
  const hasRules = policyHasRows(data.rules);

  // Nothing rule-level to show: say so with whatever sbx did give us, rather
  // than leave a blank where the rules should be. The add form still renders —
  // writing a rule doesn't depend on being able to read the current ones.
  if (!hasRules) {
    const v = data.rules || {};
    if (v.error) parts.push(`<div class="ports-empty-msg" style="color:#f85149">${escHtml(v.error)}</div>`);
    else if (v.raw) parts.push(`<pre class="policy-raw">${escHtml(v.raw)}</pre>`);
    else parts.push('<div class="ports-empty-msg">sbx reported no rules for this sandbox</div>');
  }
  parts.push(renderPolicyRules(data.rules, data.policies));

  // Only a problem when sbx *also* gave us no scope column to filter on: with
  // one, scoping the rows ourselves reaches the same answer.
  const unscoped = [data.rules, data.policies].some(v =>
    v && !v.error && v.columns && v.columns.length && !v.sandbox_scoped &&
    !(v.roles || []).some(r => r === 'applies' || r === 'sandbox'));
  if (unscoped)
    parts.push('<div class="policy-note warn">⚠ This sbx didn\'t accept a sandbox argument and its output has no scope column — some of these rules may belong to other sandboxes.</div>');

  parts.push(renderPolicyLog(data.log));
  parts.push(renderPolicySources(data.policies));

  if (data.configured_allow || data.configured_deny) {
    const allow = data.configured_allow || [];
    const deny  = data.configured_deny  || [];
    const chips =
      allow.map(d => `<span class="policy-chip">${escHtml(d)}</span>`).join('') +
      deny.map(d => `<span class="policy-chip deny">✕ ${escHtml(d)}</span>`).join('');
    parts.push(
      // Folded when the live rules already list the domains; open when they are
      // the only answer the panel has.
      `<details class="policy-details"${hasRules ? '' : ' open'}><summary>Domains sbxw allows on up — sbxw.toml (${allow.length} allow${deny.length ? `, ${deny.length} deny` : ''})</summary>` +
      (chips ? `<div class="policy-chips">${chips}</div>`
             : '<div class="ports-empty-msg">No allowlist configured</div>') +
      `</details>`
    );
  }

  const raw = (data.rules && data.rules.raw) || (data.policies && data.policies.raw);
  if (raw)
    parts.push(`<details class="policy-details"><summary>Raw <code>sbx policy ls</code> output</summary><pre class="policy-raw">${escHtml(raw)}</pre></details>`);

  policyContent.innerHTML = parts.filter(Boolean).join('');
  wirePolicyFilter();
  wirePolicyForm();
  wirePolicyDeletes();
}

// Add a rule. A host-wide one is confirmed first: it outlives this sandbox and
// governs every other, which is not what "add a rule here" usually means.
function wirePolicyForm() {
  const res    = document.getElementById('policy-add-res');
  const dec    = document.getElementById('policy-add-decision');
  const global = document.getElementById('policy-add-global');
  const btn    = document.getElementById('policy-add-btn');
  const errEl  = document.getElementById('policy-add-error');
  if (!res || !btn) return;

  const sync = () => { btn.disabled = !res.value.trim(); };
  res.addEventListener('input', sync);
  res.addEventListener('keydown', e => { if (e.key === 'Enter' && !btn.disabled) btn.click(); });

  btn.addEventListener('click', async () => {
    const resources = res.value.trim();
    const decision  = dec.value;
    const hostWide  = global.checked;
    if (!resources) return;
    if (hostWide && !confirm(
      `Add a host-wide ${decision} rule for:\n\n  ${resources}\n\n` +
      `This applies to EVERY sandbox on this host, including ones created later.`)) return;

    const target = portsTarget;
    btn.disabled = true;
    errEl.classList.add('hidden');
    const jobId = startBgJob(`Adding ${decision} rule…`);
    try {
      const r = await fetch(`/api/sandboxes/${encodeURIComponent(target)}/policy/rules`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ resources, decision, global: hostWide }),
      });
      const data = await r.json();
      if (data.ok) {
        if (portsTarget === target) await fetchPorts(target);
      } else {
        errEl.textContent = data.error || 'Failed to add rule';
        errEl.classList.remove('hidden');
      }
    } finally {
      finishBgJob(jobId);
      sync();
    }
  });
}

function wirePolicyDeletes() {
  for (const btn of policyContent.querySelectorAll('.btn-del-rule')) {
    btn.addEventListener('click', async () => {
      const { rule, resource, decision, scope } = btn.dataset;
      // Removing a rule changes what a sandbox can reach — and a host-wide rule
      // changes it for all of them. An unknown scope says so: a warning that
      // cries "every sandbox" on a guess is one people learn to click through.
      const blast = !scope ? 'sbxw can\'t tell from this listing which sandboxes it covers.'
        : scope === 'all sandboxes' ? 'It applies to EVERY sandbox on this host.'
        : `It applies to ${scope}.`;
      if (!confirm(`Remove this ${decision || 'policy'} rule?\n\n  ${resource || rule}\n\n${blast}`)) return;

      const target = portsTarget;
      btn.disabled = true;
      const jobId = startBgJob('Removing rule…');
      try {
        const r = await fetch(`/api/sandboxes/${encodeURIComponent(target)}/policy/rules/rm`, {
          method: 'POST', headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ rule }),
        });
        const data = await r.json();
        if (data.ok) {
          if (portsTarget === target) await fetchPorts(target);
        } else {
          btn.disabled = false;
          alert(`Failed to remove rule: ${data.error}`);
        }
      } finally {
        finishBgJob(jobId);
      }
    });
  }
}

// Live filter over the rule rows. They are all in the DOM already, so this is a
// plain show/hide — no refetch, and the count stays honest about how many of
// how many are showing.
function wirePolicyFilter() {
  const input = document.getElementById('policy-rule-filter');
  const wrap  = document.getElementById('policy-rules-wrap');
  const count = document.getElementById('policy-rule-count');
  if (!input || !wrap) return;
  const rows = Array.from(wrap.querySelectorAll('tbody tr'));
  input.addEventListener('input', () => {
    const q = input.value.trim().toLowerCase();
    let shown = 0;
    for (const tr of rows) {
      const hit = !q || tr.textContent.toLowerCase().includes(q);
      tr.style.display = hit ? '' : 'none';
      if (hit) shown++;
    }
    if (count) count.textContent = q ? `${shown} of ${rows.length}` : `${rows.length}`;
  });
}

async function fetchPorts(name) {
  portsTbody.innerHTML = '<tr><td colspan="4" class="ports-empty-msg">Loading…</td></tr>';
  hostsTbody.innerHTML = '<tr><td colspan="2" class="ports-empty-msg">Loading…</td></tr>';
  policyContent.innerHTML = '<div class="ports-empty-msg">Loading…</div>';
  // The policy lookup shells out to sbx, so it is settled separately: a slow or
  // failing `policy ls` must not hold back (or blank out) the ports table.
  fetch(`/api/sandboxes/${encodeURIComponent(name)}/policy`)
    .then(r => r.json())
    .then(policy => { if (portsTarget === name) renderPolicy(policy); })
    .catch(() => { if (portsTarget === name) renderPolicy(null); });
  try {
    const [portsRes, hostsRes] = await Promise.all([
      fetch(`/api/sandboxes/${encodeURIComponent(name)}/ports`),
      fetch('/api/hosts'),
    ]);
    const portsData = await portsRes.json();
    const hostsData = await hostsRes.json();
    renderPortsTable(portsData.ports || []);
    renderHostsTable(hostsData || []);
  } catch (_) {
    portsTbody.innerHTML = '<tr><td colspan="4" class="ports-empty-msg" style="color:#f85149">Error fetching ports</td></tr>';
  }
}

async function deletePort(spec, btn) {
  btn.disabled = true;
  const res = await fetch(`/api/sandboxes/${encodeURIComponent(portsTarget)}/ports/unpublish`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ spec }),
  });
  const data = await res.json();
  if (data.ok) {
    await fetchPorts(portsTarget);
  } else {
    btn.disabled = false;
    alert(`Failed to unpublish: ${data.error}`);
  }
}

addSbxPort.addEventListener('input', () => {
  btnAddMapping.disabled = !addSbxPort.value;
  if (addSbxPort.value && !addHostPort.value)
    addHostPort.placeholder = addSbxPort.value;
});

btnAddMapping.addEventListener('click', async () => {
  const sbxPort  = parseInt(addSbxPort.value, 10);
  const hostPort = parseInt(addHostPort.value, 10) || undefined;
  const hostIp   = addHostIp.value.trim() || undefined;
  const alias    = addAlias.value.trim() || undefined;
  if (!sbxPort) return;

  const body = { sandbox_port: sbxPort };
  if (hostPort) body.host_port = hostPort;
  if (hostIp)   body.host_ip   = hostIp;
  if (alias)    body.alias     = alias;

  btnAddMapping.disabled = true;
  portsAddError.classList.add('hidden');
  const jobId = startBgJob(`Publishing port ${sbxPort}…`);
  try {
    const res = await fetch(`/api/sandboxes/${encodeURIComponent(portsTarget)}/ports/publish`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    const data = await res.json();
    if (data.ok) {
      addSbxPort.value  = '';
      addHostPort.value = '';
      addHostIp.value   = '';
      addAlias.value    = '';
      addHostPort.placeholder = 'same';
      await fetchPorts(portsTarget);
      if (data.hosts_warning) {
        portsAddError.textContent = '⚠ ' + data.hosts_warning;
        portsAddError.classList.remove('hidden');
      }
    } else {
      portsAddError.textContent = data.error || 'Failed to publish';
      portsAddError.classList.remove('hidden');
    }
  } finally {
    finishBgJob(jobId);
    btnAddMapping.disabled = !addSbxPort.value;
  }
});

function openPortsModal(name) {
  portsTarget = name;
  portsNameEl.textContent = name;
  addSbxPort.value  = '';
  addHostPort.value = '';
  addHostIp.value   = '';
  addAlias.value    = '';
  addHostPort.placeholder = 'same';
  btnAddMapping.disabled = true;
  portsAddError.classList.add('hidden');
  portsOverlay.classList.remove('hidden');
  fetchPorts(name);
}

function closePortsModal() { portsOverlay.classList.add('hidden'); portsTarget = null; }

document.getElementById('ports-modal-close').addEventListener('click', closePortsModal);
document.getElementById('ports-modal-close2').addEventListener('click', closePortsModal);
document.getElementById('ports-modal-refresh').addEventListener('click', () => {
  if (portsTarget) fetchPorts(portsTarget);
});
portsOverlay.addEventListener('click', e => { if (e.target === portsOverlay) closePortsModal(); });
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && !portsOverlay.classList.contains('hidden')) closePortsModal();
});
