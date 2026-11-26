// sbxw's ticket sync: one operation against an issue tracker, with no agent
// in the loop. The daemon runs this file with `node - <request>` (the script
// on stdin) inside a small sandbox of its own, where sbx's MCP gateway is
// reachable and the tracker's MCP server is loaded: the gateway and the
// connector hold the authentication, this script holds none and reads none.
//
// The request is the one argument, JSON, maybe prefixed with one `=` (sbx
// refuses empty arguments, so values go as `=value`). The answer is the LAST
// line on stdout: `SBXW-TRACKER <json>`, `{ok: true, ...}` or
// `{ok: false, error: {kind, message, retry_after_s?}}`, kind one of auth,
// missing_server, rate_limit, not_found, network, unsupported. The exit code
// is 0 whenever an answer was printed. Diagnostics go to stderr, and never
// carry a token or a header value.
//
//   {op: "probe", tracker}                         -> {tools, identity, via, ops}
//   {op: "list", tracker, repo, label, page}       -> {issues, next_page}
//   {op: "comment", tracker, key, comment_id, body} -> {comment_id}
//   {op: "label", tracker, key, add, remove_prefix} -> {labels}
//   {op: "close", tracker, key}                    -> {}
//   {op: "fetch", tracker, url, max_bytes}         -> {path, name, bytes}
//
// Layers: a small MCP client (JSON-RPC over HTTP, JSON or event-stream
// answers), then one adapter per tracker: a table of candidate tools per
// operation, tried in order against what the gateway lists, so a tool renamed
// by a new server version is fixed by editing the table, not the code. A
// server's tools are called directly when tools/list shows them, else
// through the gateway's `mcp-exec` tool.
//
// Everything a tracker returns is data: never evaluated, never used as a path
// (file names are made here), every string and list cut to a bound.
'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('crypto');

const ANSWER_MARK = 'SBXW-TRACKER';
const PROTOCOL = '2025-06-18';
const GATEWAY_ENTRY = 'mcp-gateway';
const CALL_TIMEOUT_MS = 30000;
const FETCH_TIMEOUT_MS = 120000;
const TOTAL_TIMEOUT_MS = 300000;
const MAX_ANSWER_BYTES = 8 * 1024 * 1024;
const MAX_REQUEST_CHARS = 1000000;
const BODY_CHARS = 16000;
const BODY_SCAN_CHARS = 262144;
const TITLE_CHARS = 500;
const LABEL_CHARS = 100;
const MAX_LABELS = 100;
const COMMENT_CHARS = 65000;
const PER_PAGE = 50;
const MAX_PAGE = 40;
const MAX_ISSUES = 100;
const MAX_ATTACHMENTS = 20;
const MAX_TOOLS = 500;
const MAX_TOOL_PAGES = 20;
const MAX_REDIRECTS = 5;
const TMP_DIR = '/tmp/sbxw-tracker';
const SCHEMES = ['https:'];

// ══ Errors ════════════════════════════════════════════════════════════════
class TrackerError extends Error {
  constructor(kind, message, extra = {}) {
    super(message);
    this.kind = kind;
    Object.assign(this, extra);
  }
}
const KINDS = ['auth', 'missing_server', 'rate_limit', 'not_found', 'network', 'unsupported'];

// Anything shaped like a credential, hidden from every message and log line.
function scrub(text) {
  return String(text ?? '')
    .replace(/\b(gh[pousr]_[A-Za-z0-9]{10,}|github_pat_[A-Za-z0-9_]{10,}|glpat-[A-Za-z0-9_-]{10,})/g, '[hidden]')
    .replace(/\b(Bearer|Basic|token)\s+[A-Za-z0-9._~+/=-]{8,}/gi, '$1 [hidden]')
    .replace(/([?&](?:token|jwt|access_token|signature|X-Amz-[A-Za-z]+)=)[^&\s"']+/gi, '$1[hidden]');
}
const cut = (s, n) => {
  const chars = [...String(s ?? '')];
  return chars.length > n ? chars.slice(0, n).join('') : chars.join('');
};
const say = msg => { try { process.stderr.write(`sbxw-tracker: ${cut(scrub(msg), 2000)}\n`); } catch { /* stderr gone */ } };

// A tracker's error text to a kind. `toolMissing` is set when the text says
// the tool itself does not exist (then the next candidate is tried).
function classify(text, status) {
  const t = String(text ?? '');
  const short = cut(scrub(t.replace(/\s+/g, ' ').trim()), 300) || 'no detail';
  if (/(\btool\b|\bfunction\b)[^.\n]{0,60}(not found|unknown|does not exist|not available|no such)|unknown tool|no such tool/i.test(t)) {
    return new TrackerError('missing_server', `the tool is not there: ${short}`, { toolMissing: true });
  }
  if (status === 429 || /rate limit|too many requests|abuse detection|\b429\b/i.test(t)) {
    const m = t.match(/retry[- ]after[^0-9]{0,10}(\d{1,6})/i) || t.match(/in (\d{1,6}) ?s(?:ec|econds)?\b/i);
    return new TrackerError('rate_limit', `the tracker limits requests: ${short}`, m ? { retry_after_s: Number(m[1]) } : {});
  }
  if (status === 401 || status === 403 || /\b40[13]\b|bad credentials|unauthori[sz]ed|requires authentication|forbidden|not accessible by|token[^.\n]{0,20}(expired|revoked|invalid)|re-?authenticat|sign in again|oauth/i.test(t)) {
    return new TrackerError('auth', `the tracker refused the account: ${short}`);
  }
  if (/\b(400|422)\b|validation failed|invalid (argument|param|input|request)|missing required|required (parameter|property|argument|field)|unknown (argument|parameter|property|field)|unexpected (argument|parameter|property)|failed to parse|schema/i.test(t)) {
    return new TrackerError('unsupported', `the tool refused these arguments (edit the candidate table in tracker-sync.js): ${short}`, { badArgs: true });
  }
  if (status === 404 || /\b404\b|not found|does not exist|could not resolve to/i.test(t)) {
    return new TrackerError('not_found', `not found: ${short}`);
  }
  return new TrackerError('network', `the tracker failed: ${short}`);
}

// ══ The MCP client ════════════════════════════════════════════════════════
// The gateway's entry in ~/.claude.json, as sbx writes it:
// mcpServers["mcp-gateway"] = {type: "http", url, headers: {Authorization}}.
function readGateway(home = os.homedir()) {
  let raw;
  try { raw = fs.readFileSync(path.join(home, '.claude.json'), 'utf8'); } catch {
    throw new TrackerError('missing_server', 'no ~/.claude.json in this sandbox: the MCP gateway is not configured here');
  }
  let j;
  try { j = JSON.parse(raw); } catch { throw new TrackerError('missing_server', '~/.claude.json is not JSON'); }
  const e = j && j.mcpServers && j.mcpServers[GATEWAY_ENTRY];
  if (!e || typeof e !== 'object' || typeof e.url !== 'string') {
    throw new TrackerError('missing_server', `~/.claude.json has no mcpServers["${GATEWAY_ENTRY}"] entry with a url`);
  }
  let u;
  try { u = new URL(e.url); } catch { throw new TrackerError('missing_server', 'the MCP gateway\'s url does not parse'); }
  if (u.protocol !== 'http:' && u.protocol !== 'https:') throw new TrackerError('missing_server', 'the MCP gateway\'s url is not http(s)');
  const headers = {};
  if (e.headers && typeof e.headers === 'object') {
    for (const [k, v] of Object.entries(e.headers)) if (typeof v === 'string' && /^[A-Za-z0-9-]{1,100}$/.test(k)) headers[k] = v;
  }
  return { url: u.href, headers };
}

// Reads a response body up to MAX_ANSWER_BYTES. For an event stream, stops at
// the JSON-RPC message answering `id`; for JSON, parses the whole body.
async function readMessage(res, id) {
  const type = String(res.headers.get('content-type') || '').toLowerCase();
  const sse = type.includes('text/event-stream');
  if (!res.body) return null;
  const reader = res.body.getReader();
  const dec = new TextDecoder();
  let buf = '';
  let total = 0;
  const pick = msg => {
    for (const m of Array.isArray(msg) ? msg : [msg]) {
      if (m && typeof m === 'object' && m.id === id && ('result' in m || 'error' in m)) return m;
    }
    return null;
  };
  const event = block => {
    const data = block.split('\n').filter(l => l.startsWith('data:')).map(l => l.slice(5).replace(/^ /, '')).join('\n');
    if (!data.trim()) return null;
    try { return pick(JSON.parse(data)); } catch { return null; }
  };
  for (;;) {
    const { done, value } = await reader.read();
    if (value) {
      total += value.length;
      if (total > MAX_ANSWER_BYTES) {
        reader.cancel().catch(() => {});
        throw new TrackerError('network', 'the MCP gateway\'s answer is larger than this script reads');
      }
      buf += dec.decode(value, { stream: true });
    }
    if (sse) {
      buf = buf.replace(/\r\n/g, '\n');
      let at;
      while ((at = buf.indexOf('\n\n')) >= 0) {
        const block = buf.slice(0, at);
        buf = buf.slice(at + 2);
        const m = event(block);
        if (m) { reader.cancel().catch(() => {}); return m; }
      }
    }
    if (done) break;
  }
  if (sse) return event(buf.replace(/\r\n/g, '\n'));
  if (!buf.trim()) return null;
  try { return pick(JSON.parse(buf)); } catch { throw new TrackerError('network', 'the MCP gateway answered something that is not JSON'); }
}

class McpClient {
  constructor(gateway, o = {}) {
    this.url = gateway.url;
    this.headers = gateway.headers;
    this.timeoutMs = o.timeoutMs || CALL_TIMEOUT_MS;
    this.session = null;
    this.protocol = null;
    this.nextId = 1;
    this.ready = false;
  }

  // One POST, with the time limit; a dropped connection is tried once more.
  async post(body, id) {
    for (let attempt = 0; ; attempt++) {
      const ctl = new AbortController();
      const timer = setTimeout(() => ctl.abort(), this.timeoutMs);
      const headers = { ...this.headers, 'Content-Type': 'application/json', Accept: 'application/json, text/event-stream' };
      if (this.session) headers['Mcp-Session-Id'] = this.session;
      if (this.protocol) headers['MCP-Protocol-Version'] = this.protocol;
      try {
        const res = await fetch(this.url, { method: 'POST', headers, body: JSON.stringify(body), signal: ctl.signal });
        const sid = res.headers.get('mcp-session-id');
        if (sid && /^[\x21-\x7e]{1,200}$/.test(sid)) this.session = sid;
        const msg = res.ok && id !== undefined ? await readMessage(res, id) : (res.body && res.body.cancel().catch(() => {}), null);
        return { status: res.status, headers: res.headers, msg };
      } catch (e) {
        if (e instanceof TrackerError) throw e;
        if (ctl.signal.aborted) throw new TrackerError('network', `the MCP gateway did not answer within ${Math.round(this.timeoutMs / 1000)} s`);
        if (attempt === 0) { say('the connection to the MCP gateway dropped; trying once more'); continue; }
        throw new TrackerError('network', `the connection to the MCP gateway dropped twice: ${scrub(e.cause?.code || e.message)}`);
      } finally {
        clearTimeout(timer);
      }
    }
  }

  async request(method, params, again = false) {
    const id = this.nextId++;
    const { status, headers, msg } = await this.post({ jsonrpc: '2.0', id, method, params }, id);
    if (status === 404 && this.session && method !== 'initialize' && !again) {
      say('the MCP session ended; starting a new one');
      this.session = null;
      this.protocol = null;
      this.ready = false;
      await this.init();
      return this.request(method, params, true);
    }
    if (status === 401 || status === 403) throw new TrackerError('auth', `the MCP gateway refused the request (HTTP ${status})`);
    if (status === 429) {
      const ra = Number(headers.get('retry-after'));
      throw new TrackerError('rate_limit', 'the MCP gateway limits requests (HTTP 429)', Number.isFinite(ra) && ra >= 0 ? { retry_after_s: Math.ceil(ra) } : {});
    }
    if (status < 200 || status >= 300) throw new TrackerError('network', `the MCP gateway answered HTTP ${status}`);
    if (!msg) throw new TrackerError('network', `the MCP gateway sent no answer to ${method}`);
    if (msg.error) {
      const e = classify(`${msg.error.message || ''} ${typeof msg.error.data === 'string' ? msg.error.data : ''}`);
      if (msg.error.code === -32601 && method !== 'tools/call') throw new TrackerError('missing_server', `the MCP gateway does not know ${method}`);
      throw e;
    }
    return msg.result;
  }

  async init() {
    const r = await this.request('initialize', { protocolVersion: PROTOCOL, capabilities: {}, clientInfo: { name: 'sbxw-tracker', version: '1' } });
    const v = r && typeof r.protocolVersion === 'string' && /^[\w.-]{1,40}$/.test(r.protocolVersion) ? r.protocolVersion : PROTOCOL;
    this.protocol = v;
    await this.post({ jsonrpc: '2.0', method: 'notifications/initialized' });
    this.ready = true;
    return r;
  }

  // Every tool the gateway lists: name -> inputSchema (or null).
  async listTools() {
    const tools = new Map();
    let cursor;
    for (let i = 0; i < MAX_TOOL_PAGES; i++) {
      const r = await this.request('tools/list', cursor ? { cursor } : {});
      for (const t of Array.isArray(r && r.tools) ? r.tools : []) {
        if (tools.size >= MAX_TOOLS) break;
        if (t && typeof t.name === 'string' && /^[\w.:/-]{1,200}$/.test(t.name)) tools.set(t.name, t.inputSchema && typeof t.inputSchema === 'object' ? t.inputSchema : null);
      }
      cursor = r && typeof r.nextCursor === 'string' && r.nextCursor ? r.nextCursor : null;
      if (!cursor) break;
    }
    return tools;
  }

  // A tool's result as data: structuredContent, else its text parsed as JSON
  // when it is JSON, else {text}. An isError result is thrown, classified.
  async callTool(name, args) {
    const r = await this.request('tools/call', { name, arguments: args });
    const text = (Array.isArray(r && r.content) ? r.content : []).filter(c => c && c.type === 'text' && typeof c.text === 'string').map(c => c.text).join('\n');
    if (r && r.isError) throw classify(text);
    if (r && r.structuredContent && typeof r.structuredContent === 'object') return r.structuredContent;
    const t = text.trim();
    if (t.startsWith('{') || t.startsWith('[')) {
      try { return JSON.parse(t); } catch { /* not JSON after all */ }
    }
    return { text: t };
  }
}

// ══ Calling a tracker's tools ═════════════════════════════════════════════
// Arguments fitted to a tool's schema when the gateway gave one: keys the
// schema does not have are dropped, an enum takes its own case, a list
// becomes a comma list where a string is wanted. `ok` is false when a
// required key is missing or a key the candidate needs was dropped.
function fitArgs(args, schema, need = []) {
  const clean = {};
  for (const [k, v] of Object.entries(args)) if (v !== undefined && v !== null) clean[k] = v;
  const props = schema && schema.properties && typeof schema.properties === 'object' ? schema.properties : null;
  if (!props) return { ok: true, args: clean };
  const out = {};
  for (const [k, v] of Object.entries(clean)) {
    if (!Object.prototype.hasOwnProperty.call(props, k)) continue;
    out[k] = coerce(v, props[k] || {});
  }
  const required = Array.isArray(schema.required) ? schema.required : [];
  const missing = [...required.filter(r => !(r in out)), ...need.filter(r => !(r in out))];
  return { ok: !missing.length, args: out, missing };
}
function coerce(v, prop) {
  const type = Array.isArray(prop.type) ? prop.type[0] : prop.type;
  if (typeof v === 'string' && Array.isArray(prop.enum)) {
    const hit = prop.enum.find(e => typeof e === 'string' && e.toLowerCase() === v.toLowerCase());
    if (hit) return hit;
  }
  if (Array.isArray(v) && type === 'string') return v.join(',');
  if (typeof v === 'number' && type === 'string') return String(v);
  if (typeof v === 'string' && (type === 'integer' || type === 'number') && /^\d{1,15}$/.test(v)) return Number(v);
  return v;
}

class Session {
  constructor(client, adapter) {
    this.client = client;
    this.adapter = adapter;
    this.tools = null;
    this.via = null;
    this.used = {};
  }

  async open() {
    if (this.tools) return;
    await this.client.init();
    this.tools = await this.client.listTools();
    say(`tools listed by the gateway: ${[...this.tools.keys()].slice(0, 60).join(', ') || 'none'}`);
    if (this.adapter.knownTools().some(n => this.tools.has(n))) this.via = 'direct';
    else if (this.tools.has('mcp-exec')) this.via = 'mcp-exec?';
  }

  // The first candidate of `op` the gateway offers whose schema fits, or null.
  resolve(op, p) {
    for (const c of this.adapter.ops[op] || []) {
      if (!this.tools.has(c.tool)) continue;
      const fit = fitArgs(c.args(p), this.tools.get(c.tool), c.need);
      if (fit.ok) return { c, args: fit.args };
      say(`${c.tool} is listed but its schema lacks ${fit.missing.join(', ')}: next candidate`);
    }
    return null;
  }

  // Runs `op` with the parameters `p`: its candidates in order, directly when
  // listed, else through mcp-exec. A candidate whose arguments are refused
  // gives way to the next; any other failure is the answer.
  async run(op, p) {
    await this.open();
    const cands = this.adapter.ops[op] || [];
    if (!cands.length) throw new TrackerError('unsupported', `the ${this.adapter.name} server has no tool for ${op}`);
    let last = null;
    if (this.via === 'direct') {
      for (const c of cands) {
        if (!this.tools.has(c.tool)) continue;
        const fit = fitArgs(c.args(p), this.tools.get(c.tool), c.need);
        if (!fit.ok) { say(`${c.tool} is listed but its schema lacks ${fit.missing.join(', ')}: next candidate`); continue; }
        try {
          const data = await this.client.callTool(c.tool, fit.args);
          this.used[op] = c.tool;
          return c.read ? c.read(data, p) : data;
        } catch (e) {
          if (!(e instanceof TrackerError) || !(e.badArgs || e.toolMissing)) throw e;
          say(`${c.tool}: ${e.message}: next candidate`);
          last = e;
        }
      }
      if (last && last.badArgs) throw last;
      throw new TrackerError('unsupported', `the ${this.adapter.name} server lists none of ${cands.map(c => c.tool).join(', ')} (for ${op}) with arguments this script knows`);
    }
    if (this.via && this.via.startsWith('mcp-exec')) {
      for (const c of cands) {
        try {
          const data = await this.client.callTool('mcp-exec', { name: c.tool, arguments: fitArgs(c.args(p), null).args });
          this.via = 'mcp-exec';
          this.used[op] = c.tool;
          return c.read ? c.read(data, p) : data;
        } catch (e) {
          if (!(e instanceof TrackerError) || !(e.badArgs || e.toolMissing)) throw e;
          say(`mcp-exec ${c.tool}: ${e.message}: next candidate`);
          last = e;
        }
      }
      if (last && last.badArgs) throw last;
      if (this.via === 'mcp-exec' || await this.execWorks()) {
        throw new TrackerError('unsupported', `the ${this.adapter.name} server offers none of ${cands.map(c => c.tool).join(', ')} (for ${op})`);
      }
    }
    throw new TrackerError('missing_server', `the ${this.adapter.name} MCP server is not loaded in this sandbox's gateway (none of its tools is listed${this.tools.has('mcp-exec') ? ', and mcp-exec does not reach it' : ''})`);
  }

  // True when mcp-exec reaches the tracker's server: one cheap read.
  async execWorks() {
    const c = (this.adapter.ops.identity || [])[0];
    if (!c) return false;
    try {
      await this.client.callTool('mcp-exec', { name: c.tool, arguments: c.args({}) });
      this.via = 'mcp-exec';
      return true;
    } catch (e) {
      if (e instanceof TrackerError && e.kind === 'auth') throw e;
      return false;
    }
  }
}

// ══ GitHub ════════════════════════════════════════════════════════════════
// GitHub's official MCP server (github/github-mcp-server). Its tools have
// been renamed between versions: issue_read / get_issue, issue_write /
// update_issue, label_write / create_label. Each operation lists its
// candidates in order; the first the gateway offers with a fitting schema
// is used. UNTESTED against the real server: run `probe` on the person's
// machine and fix this table from what it prints.
const GITHUB_ATTACH_HOSTS = ['github.com', 'user-images.githubusercontent.com', 'private-user-images.githubusercontent.com'];
const GITHUB_FETCH_HOSTS = ['github.com', 'user-images.githubusercontent.com', 'private-user-images.githubusercontent.com', 'objects.githubusercontent.com', 'raw.githubusercontent.com'];

const ghKey = p => ({ owner: p.owner, repo: p.repo, issue_number: p.number });

function ghLogin(d) {
  const l = d && (d.login || (d.user && d.user.login) || (d.viewer && d.viewer.login) || (d.details && d.details.login));
  return typeof l === 'string' && /^[A-Za-z0-9-]{1,39}(\[bot\])?$/.test(l) ? l : null;
}

const GITHUB = {
  name: 'github',
  attachHosts: GITHUB_ATTACH_HOSTS,
  fetchHosts: GITHUB_FETCH_HOSTS,
  ops: {
    identity: [
      { tool: 'get_me', args: () => ({}), read: ghLogin },
    ],
    list: [
      { tool: 'list_issues', need: ['owner', 'repo'],
        args: p => ({ owner: p.owner, repo: p.repo, state: 'open', labels: [p.label], perPage: PER_PAGE, per_page: PER_PAGE,
          page: typeof p.page === 'number' ? p.page : undefined, after: typeof p.page === 'string' ? p.page : undefined,
          orderBy: 'UPDATED_AT', direction: 'DESC' }) },
      { tool: 'search_issues', need: ['query'],
        args: p => ({ query: `repo:${p.owner}/${p.repo} is:issue is:open label:"${p.label}"`, perPage: PER_PAGE, per_page: PER_PAGE,
          page: typeof p.page === 'number' ? p.page : undefined, sort: 'updated', order: 'desc' }) },
    ],
    get: [
      { tool: 'issue_read', need: ['method'], args: p => ({ method: 'get', ...ghKey(p) }) },
      { tool: 'get_issue', args: p => ghKey(p) },
    ],
    comment_create: [
      { tool: 'add_issue_comment', need: ['body'], args: p => ({ ...ghKey(p), body: p.body }) },
    ],
    // The official server is not known to update a comment: these names are
    // guesses kept so a future version is picked up; else `unsupported`.
    comment_update: [
      { tool: 'update_issue_comment', need: ['body'], args: p => ({ owner: p.owner, repo: p.repo, comment_id: p.comment_id, body: p.body }) },
      { tool: 'edit_issue_comment', need: ['body'], args: p => ({ owner: p.owner, repo: p.repo, comment_id: p.comment_id, body: p.body }) },
    ],
    set_labels: [
      { tool: 'issue_write', need: ['method', 'labels'], args: p => ({ method: 'update', ...ghKey(p), labels: p.labels }) },
      { tool: 'update_issue', need: ['labels'], args: p => ({ ...ghKey(p), labels: p.labels }) },
    ],
    create_label: [
      { tool: 'label_write', need: ['method', 'name'], args: p => ({ method: 'create', owner: p.owner, repo: p.repo, name: p.name, color: 'ededed', description: 'Set by sbxw' }) },
      { tool: 'create_label', need: ['name'], args: p => ({ owner: p.owner, repo: p.repo, name: p.name, color: 'ededed', description: 'Set by sbxw' }) },
    ],
    close: [
      { tool: 'issue_write', need: ['method', 'state'], args: p => ({ method: 'update', ...ghKey(p), state: 'closed', state_reason: 'completed' }) },
      { tool: 'update_issue', need: ['state'], args: p => ({ ...ghKey(p), state: 'closed', state_reason: 'completed' }) },
    ],
    // No tool of the official server downloads an issue's attachment.
    download: [],
  },
  knownTools() {
    return [...new Set(Object.values(this.ops).flat().map(c => c.tool))];
  },
  parseRepo(s) {
    const m = typeof s === 'string' && s.match(/^([A-Za-z0-9](?:[A-Za-z0-9-]{0,38}))\/([A-Za-z0-9._-]{1,100})$/);
    if (!m || m[2] === '.' || m[2] === '..') throw new TrackerError('unsupported', 'repo must be "owner/name"');
    return { owner: m[1], repo: m[2] };
  },
  parseKey(s) {
    const m = typeof s === 'string' && s.match(/^([A-Za-z0-9](?:[A-Za-z0-9-]{0,38}))\/([A-Za-z0-9._-]{1,100})#(\d{1,9})$/);
    if (!m || m[2] === '.' || m[2] === '..') throw new TrackerError('unsupported', 'key must be "owner/name#number"');
    return { owner: m[1], repo: m[2], number: Number(m[3]) };
  },
  issueUrl: (owner, repo, n) => `https://github.com/${owner}/${repo}/issues/${n}`,
};

const ADAPTERS = { github: GITHUB };

// ══ Issues as data ════════════════════════════════════════════════════════
const str = v => (typeof v === 'string' ? v : '');

function labelNames(v) {
  const list = Array.isArray(v) ? v : v && Array.isArray(v.nodes) ? v.nodes : null;
  if (!list) return null;
  const out = [];
  for (const l of list) {
    const n = typeof l === 'string' ? l : l && typeof l.name === 'string' ? l.name : '';
    if (n && out.length < MAX_LABELS) out.push(cut(n, LABEL_CHARS));
  }
  return out;
}

// A safe display name for a file: no separator, no control character, no
// leading dot; never used as a path.
function safeName(s) {
  let n = '';
  try { n = decodeURIComponent(String(s ?? '')); } catch { n = String(s ?? ''); }
  n = n.replace(/[\u0000-\u001f\u007f/\\:*?"<>|]/g, '_').replace(/^[.\s_]+/, '').trim();
  n = cut(n, 120);
  return n || 'attachment';
}

// The images and files an issue's body links on the tracker's own hosts:
// markdown links and images, <img src>, and bare addresses. Bounded.
function extractAttachments(body, hosts) {
  const text = String(body ?? '').slice(0, BODY_SCAN_CHARS);
  const names = new Map();
  for (const m of text.matchAll(/!?\[([^\]\n]{0,200})\]\((https:\/\/[^\s)]{1,2000})\)/g)) if (!names.has(m[2])) names.set(m[2], m[1]);
  for (const m of text.matchAll(/<img\b[^>]{0,1000}>/gi)) {
    const src = m[0].match(/\bsrc\s*=\s*"(https:\/\/[^"]{1,2000})"/i);
    const alt = m[0].match(/\balt\s*=\s*"([^"]{0,200})"/i);
    if (src && alt && !names.has(src[1])) names.set(src[1], alt[1]);
  }
  const out = [];
  const seen = new Set();
  for (const m of text.matchAll(/https:\/\/[^\s)"'<>\]]{1,2000}/g)) {
    if (out.length >= MAX_ATTACHMENTS) break;
    const raw = m[0].replace(/[.,;!]+$/, '');
    let u;
    try { u = new URL(raw); } catch { continue; }
    if (u.username || u.password || !hosts.includes(u.host)) continue;
    if (u.host === 'github.com' && !/^\/(user-attachments\/(assets|files)\/|[^/]+\/[^/]+\/(assets|files)\/)/.test(u.pathname)) continue;
    if (seen.has(u.href)) continue;
    seen.add(u.href);
    const label = names.get(raw) || names.get(m[0]);
    const last = u.pathname.split('/').filter(Boolean).pop() || '';
    const generic = !label || /^(image|img|screenshot|file|attachment)$/i.test(label.trim());
    out.push({ name: safeName(generic && /\.[A-Za-z0-9]{1,8}$/.test(last) ? last : label || last), url: u.href });
  }
  return out;
}

// One issue from a tool's answer, in the contract's shape, or null when it
// is not an issue (a pull request, no number). The url is built here from
// the repo and the number, never taken from the answer.
function normIssue(i, adapter, owner, repo) {
  if (!i || typeof i !== 'object') return null;
  if (i.pull_request || i.pullRequest || i.isPullRequest) return null;
  const number = typeof i.number === 'number' ? i.number : /^\d{1,9}$/.test(str(i.number)) ? Number(i.number) : NaN;
  if (!Number.isInteger(number) || number <= 0 || number >= 1e9) return null;
  const raw = str(i.body).slice(0, BODY_SCAN_CHARS);
  const state = str(i.state).toLowerCase();
  const updated = str(i.updated_at) || str(i.updatedAt);
  return {
    key: `${owner}/${repo}#${number}`,
    number,
    title: cut(str(i.title), TITLE_CHARS),
    body: cut(raw, BODY_CHARS),
    labels: labelNames(i.labels),
    state: state === 'open' || state === 'closed' ? state : '',
    url: adapter.issueUrl(owner, repo, number),
    updated_at: /^\d{4}-\d\d-\d\dT[\d:.]{5,15}(Z|[+-]\d\d:?\d\d)?$/.test(updated) ? updated : '',
    attachments: extractAttachments(raw, adapter.attachHosts),
  };
}

function readIssueList(data, p, adapter) {
  const list = Array.isArray(data) ? data
    : Array.isArray(data && data.issues) ? data.issues
      : Array.isArray(data && data.items) ? data.items
        : Array.isArray(data && data.nodes) ? data.nodes : null;
  if (!list) throw new TrackerError('unsupported', 'the list tool answered in a shape this script does not read');
  const issues = [];
  for (const raw of list.slice(0, MAX_ISSUES * 2)) {
    const i = normIssue(raw, adapter, p.owner, p.repo);
    if (!i) continue;
    if (i.state && i.state !== 'open') continue;
    if (i.labels && !i.labels.some(l => l.toLowerCase() === p.label.toLowerCase())) continue;
    if (!i.labels) i.labels = [];
    if (issues.length < MAX_ISSUES) issues.push(i);
  }
  const info = data && (data.pageInfo || data.page_info);
  let next = null;
  const page = typeof p.page === 'number' ? p.page : 1;
  if (info && typeof info === 'object') {
    if (info.hasNextPage && typeof info.endCursor === 'string' && /^[\x21-\x7e]{1,500}$/.test(info.endCursor)) next = info.endCursor;
  } else if (data && typeof data.total_count === 'number') {
    if (page * PER_PAGE < data.total_count && page < MAX_PAGE) next = page + 1;
  } else if (list.length >= PER_PAGE && page < MAX_PAGE) {
    next = page + 1;
  }
  return { issues, next_page: next };
}

// ══ Operations ════════════════════════════════════════════════════════════
const okLabel = s => typeof s === 'string' && s.length >= 1 && [...s].length <= 50 && !/[\u0000-\u001f\u007f"]/.test(s);

async function opProbe(sess) {
  await sess.open();
  const tools = [...sess.tools.keys()];
  let identity = null;
  try {
    identity = await sess.run('identity', {});
  } catch (e) {
    if (e.kind === 'auth' || e.kind === 'rate_limit' || e.kind === 'network') throw e;
    if (e.kind === 'missing_server') {
      throw new TrackerError('missing_server', `${e.message}. Tools seen: ${tools.slice(0, 30).join(', ') || 'none'}`);
    }
    say(`no identity: ${e.message}`);
  }
  const ops = {};
  for (const op of Object.keys(sess.adapter.ops)) {
    if (sess.via === 'direct') ops[op] = sess.resolve(op, { owner: 'o', repo: 'r', number: 1, label: 'l', body: 'b', comment_id: 1, labels: ['l'], name: 'l' })?.c.tool || null;
    else ops[op] = null;
  }
  return { tools, identity, via: sess.via === 'mcp-exec?' ? null : sess.via, ops };
}

async function opList(sess, req) {
  const { owner, repo } = sess.adapter.parseRepo(req.repo);
  if (!okLabel(req.label)) throw new TrackerError('unsupported', 'label must be 1 to 50 characters, no quote or control character');
  let page = req.page;
  if (page === undefined || page === null || page === '' || page === 0) page = 1;
  if (typeof page === 'string' && /^\d{1,4}$/.test(page)) page = Number(page);
  if (typeof page === 'number' && (!Number.isInteger(page) || page < 1)) throw new TrackerError('unsupported', 'page must be a positive number or a cursor');
  if (typeof page === 'string' && !/^[\x21-\x7e]{1,500}$/.test(page)) throw new TrackerError('unsupported', 'page cursor is not one this script gave');
  if (typeof page !== 'number' && typeof page !== 'string') throw new TrackerError('unsupported', 'page must be a positive number or a cursor');
  if (typeof page === 'number' && page > MAX_PAGE) return { issues: [], next_page: null };
  const p = { owner, repo, label: req.label, page };
  const data = await sess.run('list', p);
  return readIssueList(data, p, sess.adapter);
}

function commentId(d) {
  const id = d && (d.id ?? (d.comment && d.comment.id));
  if ((typeof id === 'number' && Number.isFinite(id)) || (typeof id === 'string' && /^[A-Za-z0-9_=-]{1,100}$/.test(id))) return String(id);
  const url = d && (str(d.html_url) || str(d.url) || str(d.text));
  const m = url.match(/#issuecomment-(\d{1,15})/);
  return m ? m[1] : null;
}

async function opComment(sess, req) {
  const k = sess.adapter.parseKey(req.key);
  if (typeof req.body !== 'string' || !req.body.trim()) throw new TrackerError('unsupported', 'body must be a non-empty string');
  const body = cut(req.body, COMMENT_CHARS);
  const given = req.comment_id;
  if (given !== undefined && given !== null && given !== '') {
    if (!/^[A-Za-z0-9_=-]{1,100}$/.test(String(given))) throw new TrackerError('unsupported', 'comment_id is not an id');
    await sess.run('comment_update', { ...k, comment_id: String(given), body });
    return { comment_id: String(given) };
  }
  const d = await sess.run('comment_create', { ...k, body });
  const id = commentId(d);
  if (!id) throw new TrackerError('unsupported', 'the comment was made but the tool did not say its id');
  return { comment_id: id };
}

async function opLabel(sess, req) {
  const k = sess.adapter.parseKey(req.key);
  if (!okLabel(req.add)) throw new TrackerError('unsupported', 'add must be a label of 1 to 50 characters, no quote or control character');
  const prefix = typeof req.remove_prefix === 'string' ? req.remove_prefix : '';
  const issue = await sess.run('get', k);
  const now = labelNames(issue && (issue.labels || (issue.issue && issue.issue.labels)));
  if (!now) throw new TrackerError('unsupported', 'the issue tool did not say the issue\'s labels');
  const keep = now.filter(l => l.toLowerCase() === req.add.toLowerCase() || !prefix || !l.startsWith(prefix));
  const want = keep.some(l => l.toLowerCase() === req.add.toLowerCase()) ? keep : [...keep, req.add];
  const same = want.length === now.length && want.every(l => now.includes(l));
  if (same) return { labels: want };
  try {
    await sess.run('set_labels', { ...k, labels: want });
  } catch (e) {
    if (!(e instanceof TrackerError) || !/label/i.test(e.message) || e.kind === 'auth' || e.kind === 'rate_limit' || e.kind === 'network') throw e;
    say(`setting the labels failed (${e.message}); creating "${req.add}" first`);
    await sess.run('create_label', { owner: k.owner, repo: k.repo, name: req.add });
    await sess.run('set_labels', { ...k, labels: want });
  }
  return { labels: want };
}

async function opClose(sess, req) {
  const k = sess.adapter.parseKey(req.key);
  await sess.run('close', k);
  return {};
}

// ══ fetch: one attachment to a new file ═══════════════════════════════════
function onHosts(u, hosts) {
  return SCHEMES.includes(u.protocol) && !u.username && !u.password && hosts.includes(u.host);
}

const EXT_OF_TYPE = { 'image/png': '.png', 'image/jpeg': '.jpg', 'image/gif': '.gif', 'image/webp': '.webp', 'image/svg+xml': '.svg', 'application/pdf': '.pdf', 'text/plain': '.txt', 'video/mp4': '.mp4', 'application/zip': '.zip' };

function tmpDir() {
  fs.mkdirSync(TMP_DIR, { recursive: true, mode: 0o700 });
  const st = fs.lstatSync(TMP_DIR);
  if (!st.isDirectory() || st.isSymbolicLink()) throw new TrackerError('network', `${TMP_DIR} is not a directory`);
  return TMP_DIR;
}

async function opFetch(sess, req) {
  const a = sess.adapter;
  const max = req.max_bytes;
  if (!Number.isInteger(max) || max <= 0 || max > 1024 * 1024 * 1024) throw new TrackerError('unsupported', 'max_bytes must be a positive whole number');
  let u;
  try { u = new URL(String(req.url ?? '')); } catch { throw new TrackerError('unsupported', 'url does not parse'); }
  if (!onHosts(u, a.fetchHosts)) throw new TrackerError('unsupported', `refused: ${u.protocol}//${u.host} is not one of ${a.name}'s hosts`);
  if (a.ops.download.length) {
    // A connector that downloads would be called here; GitHub's has none.
    return sess.run('download', { url: u.href, max_bytes: max });
  }
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), FETCH_TIMEOUT_MS);
  let file = null;
  let fd = null;
  try {
    let res;
    for (let hop = 0; ; hop++) {
      try {
        res = await fetch(u.href, { redirect: 'manual', headers: { 'User-Agent': 'sbxw-tracker', Accept: '*/*' }, signal: ctl.signal });
      } catch (e) {
        if (ctl.signal.aborted) throw new TrackerError('network', 'the download took too long');
        throw new TrackerError('network', `the download failed: ${scrub(e.cause?.code || e.message)}`);
      }
      if (res.status >= 300 && res.status < 400) {
        res.body && res.body.cancel().catch(() => {});
        const loc = res.headers.get('location');
        if (!loc || hop >= MAX_REDIRECTS) throw new TrackerError('network', 'the download redirected too often, or nowhere');
        let next;
        try { next = new URL(loc, u); } catch { throw new TrackerError('unsupported', 'the download redirected to an address that does not parse'); }
        if (!onHosts(next, a.fetchHosts)) throw new TrackerError('unsupported', `refused: the download redirected off ${a.name}'s hosts, to ${next.protocol}//${next.host}`);
        u = next;
        continue;
      }
      break;
    }
    if (res.status === 401 || res.status === 403 || res.status === 404) {
      res.body && res.body.cancel().catch(() => {});
      throw new TrackerError('unsupported', `the file was refused without signing in (HTTP ${res.status}): it is likely in a private repository, and the ${a.name} connector offers no download`);
    }
    if (res.status === 429) {
      const ra = Number(res.headers.get('retry-after'));
      throw new TrackerError('rate_limit', 'the download is rate limited', Number.isFinite(ra) && ra >= 0 ? { retry_after_s: Math.ceil(ra) } : {});
    }
    if (res.status === 410) throw new TrackerError('not_found', 'the file is gone (HTTP 410)');
    if (!res.ok) throw new TrackerError('network', `the download answered HTTP ${res.status}`);
    const len = Number(res.headers.get('content-length'));
    if (Number.isFinite(len) && len > max) {
      res.body && res.body.cancel().catch(() => {});
      throw new TrackerError('unsupported', `the file is ${len} bytes, over the ${max} allowed`);
    }
    const name = safeName(u.pathname.split('/').filter(Boolean).pop() || '');
    const type = String(res.headers.get('content-type') || '').split(';')[0].trim().toLowerCase();
    const ext = (name.match(/\.[A-Za-z0-9]{1,8}$/) || [EXT_OF_TYPE[type] || '.bin'])[0].toLowerCase();
    file = path.join(tmpDir(), `${crypto.randomUUID()}${ext}`);
    fd = fs.openSync(file, 'wx', 0o600);
    let bytes = 0;
    if (res.body) {
      const reader = res.body.getReader();
      for (;;) {
        let r;
        try { r = await reader.read(); } catch (e) {
          if (ctl.signal.aborted) throw new TrackerError('network', 'the download took too long');
          throw new TrackerError('network', `the download dropped: ${scrub(e.cause?.code || e.message)}`);
        }
        if (r.done) break;
        bytes += r.value.length;
        if (bytes > max) {
          reader.cancel().catch(() => {});
          throw new TrackerError('unsupported', `the file is over the ${max} bytes allowed: stopped there`);
        }
        fs.writeSync(fd, r.value);
      }
    }
    fs.closeSync(fd);
    fd = null;
    const done = file;
    file = null;
    return { path: done, name, bytes };
  } finally {
    clearTimeout(timer);
    if (fd !== null) { try { fs.closeSync(fd); } catch { /* closed */ } }
    if (file) { try { fs.unlinkSync(file); } catch { /* never made */ } }
  }
}

// ══ The request and the answer ════════════════════════════════════════════
function parseRequest(arg) {
  if (typeof arg !== 'string' || !arg) throw new TrackerError('unsupported', 'no request: run as `node - <request json>`');
  const text = arg.startsWith('=') ? arg.slice(1) : arg;
  if (text.length > MAX_REQUEST_CHARS) throw new TrackerError('unsupported', 'the request is too large');
  let req;
  try { req = JSON.parse(text); } catch { throw new TrackerError('unsupported', 'the request is not JSON'); }
  if (!req || typeof req !== 'object' || Array.isArray(req)) throw new TrackerError('unsupported', 'the request is not an object');
  return req;
}

const OPS = { probe: opProbe, list: opList, comment: opComment, label: opLabel, close: opClose, fetch: opFetch };

async function handle(req, o = {}) {
  const op = OPS[req.op];
  if (!op || !Object.prototype.hasOwnProperty.call(OPS, req.op)) throw new TrackerError('unsupported', `unknown op ${JSON.stringify(cut(String(req.op), 40))}`);
  const adapter = Object.prototype.hasOwnProperty.call(ADAPTERS, req.tracker) ? ADAPTERS[req.tracker] : null;
  if (!adapter) throw new TrackerError('unsupported', `unknown tracker ${JSON.stringify(cut(String(req.tracker), 40))}`);
  // fetch reaches the gateway only when the adapter downloads through it.
  const needsGateway = req.op !== 'fetch' || adapter.ops.download.length > 0;
  const sess = needsGateway ? new Session(new McpClient(readGateway(o.home), o), adapter) : { adapter };
  return op(sess, req);
}

function errorOf(e) {
  if (e instanceof TrackerError && KINDS.includes(e.kind)) {
    const err = { kind: e.kind, message: cut(scrub(e.message), 600) };
    if (Number.isFinite(e.retry_after_s)) err.retry_after_s = Math.max(0, Math.min(86400, Math.round(e.retry_after_s)));
    return err;
  }
  return { kind: 'network', message: cut(scrub(`internal error: ${e && e.message}`), 600) };
}

let answered = false;
function answer(obj) {
  if (answered) return;
  answered = true;
  process.stdout.write(`\n${ANSWER_MARK} ${JSON.stringify(obj)}\n`, () => process.exit(0));
}

async function main() {
  const limit = setTimeout(() => answer({ ok: false, error: { kind: 'network', message: `no answer within ${TOTAL_TIMEOUT_MS / 1000} s` } }), TOTAL_TIMEOUT_MS);
  limit.unref();
  try {
    const req = parseRequest(process.argv[2]);
    answer({ ok: true, ...await handle(req) });
  } catch (e) {
    if (!(e instanceof TrackerError)) say(`internal error: ${e && e.stack}`);
    answer({ ok: false, error: errorOf(e) });
  }
}

if (typeof module !== 'undefined' && (require.main === module || process.argv[1] === '-')) main();
if (typeof module !== 'undefined') {
  module.exports = { McpClient, Session, GITHUB, fitArgs, classify, extractAttachments, normIssue, readIssueList, safeName, scrub, parseRequest, readGateway };
}
