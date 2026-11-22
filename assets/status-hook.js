// sbxw status hook — forwards Claude Code hook events to the sbxw daemon on the
// host so the island can show *trusted, structured* session state instead of
// scraping the terminal.
//
// It never blocks Claude Code, never influences a tool decision, and always
// exits 0. Its stdout is empty ("no decision, normal flow") except when the
// daemon answers with a note for the agent's context — a low host battery,
// told to a Remote Control session on its next prompt — which is printed as
// the daemon wrote it.
// The daemon port is templated in at install time (see sbx::install_status_hooks).
const http = require("http");
const os = require("os");
const fs = require("fs");

const PORT = __PORT__;

// The sandbox's agent token, installed by sbxw at ~/.sbxw/token. The daemon
// derives which sandbox is calling from it — a call cannot speak for another
// sandbox by naming it. Read per call, so a token refreshed by `sbxw up` is
// picked up without restarting anything.
function agentToken() {
  try {
    return require("fs").readFileSync(require("path").join(require("os").homedir(), ".sbxw", "token"), "utf8").trim();
  } catch (_) {
    return "";
  }
}

// POC diagnostics: append what happens to a local file the user can inspect
// with `sbx exec <name> -- cat /tmp/sbxw-hook.log`. Errors are otherwise silent.
function log(msg) {
  try {
    fs.appendFileSync("/tmp/sbxw-hook.log", new Date().toISOString() + " " + msg + "\n");
  } catch (_) {}
}

function done() {
  process.exit(0);
}

// What this hook can report, so the daemon can tell "the session is not over
// SSH" from "the hook is too old to have looked". Bump when a signal is added.
// v3: the transcript's tail (see `postHistory`) and the agent process.
// v4: Claude Code's session kind (`session_kind`), and `live_procs` covering
// every Claude-like process (`live_scope: "claude"`).
const HOOK_VERSION = 4;

// Which sshd-set variables are in this session's environment.
//
// This is how a session's *origin* is established rather than guessed. One
// container can run several Claude Code sessions — the one sbxw attached
// through its own PTY, and any started over SSH (Claude Desktop, an editor, a
// shell on <name>.sbx). sshd sets these per session, the hook inherits them
// through claude, and sbxw's own session has none of them.
//
// SSH_AUTH_SOCK is deliberately **not** in the list: sbx forwards an ssh agent
// into every sandbox, so it is set whether or not anyone came in over SSH, and
// keying on it would label every session `ssh`.
//
// Names only, never values: SSH_CONNECTION carries the client's IP and ports,
// and the daemon only needs to know the variable is there.
function sshEnvNames() {
  return ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"].filter(
    (k) => typeof process.env[k] === "string" && process.env[k] !== "",
  );
}

// Names of this process's ancestors, innermost first ("node", "claude", …).
//
// Kept as a secondary signal and for diagnostics. It is often *not* usable: in
// a sandbox the agent is commonly reparented, so the chain stops at "claude"
// with a ppid of 1 or 0 and says nothing about who started it. The environment
// above is what actually decides.
//
// Read from /proc, walking PPid up to init. `comm` sits between the first "("
// and the *last* ")" of /proc/<pid>/stat because a process name may itself
// contain parentheses; the state letter follows it, then PPid. Best-effort
// throughout: an unreadable chain yields an empty list. Each comes with its
// pid and start time (field 22 of its stat, in clock ticks since boot):
// `<pid>:<start>` names one process for good, a reused pid being another
// start.
function ancestors() {
  const out = [];
  let pid = process.pid;
  for (let depth = 0; depth < 24 && pid > 1; depth++) {
    let stat;
    try {
      stat = fs.readFileSync("/proc/" + pid + "/stat", "utf8");
    } catch (_) {
      break;
    }
    const open = stat.indexOf("(");
    const close = stat.lastIndexOf(")");
    if (open < 0 || close < open) break;
    // After ")" comes " <state> <ppid> …".
    const after = stat.slice(close + 1).trim().split(/\s+/);
    out.push({ pid, name: stat.slice(open + 1, close), start: after[19] || "" });
    const ppid = parseInt(after[1], 10);
    if (!Number.isFinite(ppid) || ppid <= 0) break;
    pid = ppid;
  }
  return out;
}

// BEGIN procs
// What runs only a command for the agent, not the agent itself.
const LAUNCHERS = new Set(["sh", "bash", "dash", "zsh", "fish", "env", "timeout"]);

// The agent process this hook runs for: its nearest ancestor that is not a
// shell. One Claude Code process runs one session at a time, so a new
// session from the same process ends the previous one for sure.
function agentProcess(chain) {
  const a = chain.slice(1).find((p) => !LAUNCHERS.has(p.name));
  return a && a.start ? a : null;
}

// A Claude Code process: `claude`, or its binary started by its versioned
// path (`2.1.328`), which is how Claude Code starts its background sessions.
function claudeLike(name) {
  return name === "claude" || /^[0-9]+(\.[0-9]+)+$/.test(name);
}

// Every process alive in the sandbox with the agent's name or a Claude-like
// one, as `<pid>:<start>`: which earlier sessions' processes are gone. The
// daemon is told the list covers every Claude-like name (`live_scope`), so
// a background session's start no longer reads the terminals' `claude` as
// gone. Names and pids only. Undefined when /proc cannot be listed (then
// nothing is retired).
function liveProcs(name, root) {
  root = root || "/proc";
  try {
    const out = [];
    for (const d of fs.readdirSync(root)) {
      if (!/^[0-9]+$/.test(d)) continue;
      let stat;
      try {
        stat = fs.readFileSync(root + "/" + d + "/stat", "utf8");
      } catch (_) {
        continue;
      }
      const open = stat.indexOf("(");
      const close = stat.lastIndexOf(")");
      if (open < 0 || close < open) continue;
      const comm = stat.slice(open + 1, close);
      if (comm !== name && !claudeLike(comm)) continue;
      const after = stat.slice(close + 1).trim().split(/\s+/);
      if (after[19]) out.push(d + ":" + after[19]);
    }
    return out;
  } catch (_) {
    return undefined;
  }
}

// Claude Code's own word for the session (`CLAUDE_CODE_SESSION_KIND`: `bg`
// for its background sessions), when it is a plain short word.
function sessionKind(env) {
  const k = env.CLAUDE_CODE_SESSION_KIND;
  return typeof k === "string" && /^[a-z_-]{1,20}$/.test(k) ? k : undefined;
}
// END procs
// BEGIN history
// The conversation, for the Monitor: this session's own transcript (the path
// Claude Code hands every hook; nothing else is read), from where the last
// call stopped, at most READ_MAX bytes per event, complete lines only. What
// is posted is summarised: texts clipped, a tool's few telling input fields,
// a result's status, size and head. Never thinking, images, whole results.
// The daemon masks and stores it in memory; the offset file below holds
// offsets only, never text. Any error: nothing is posted, the hook goes on.
const HISTORY_DIR = "/tmp/sbxw-history";
const READ_MAX = 256 * 1024; // new transcript bytes read per event
const BACKFILL_MAX = 2 * 1024 * 1024; // a first read starts this far from the end
const SCAN_MAX = 8 * 1024 * 1024; // an oversized line is skipped up to this
const POST_ITEMS = 380; // under the daemon's 400
const POST_BYTES = 400 * 1024; // under the daemon's 512 KB
const TEXT_MAX = 16 * 1024;
const HEAD_MAX = 2000;
const FIELD_MAX = 500;
const COMMAND_MAX = 1000;

function clipText(s, n) {
  s = typeof s === "string" ? s : "";
  return s.length > n ? s.slice(0, n) : s;
}

function validSession(sid) {
  return typeof sid === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(sid);
}

// Why the transcript cannot be read, or "" when it can.
function transcriptProblem(evt) {
  const tp = evt.transcript_path;
  if (typeof tp !== "string" || tp === "") return "no_path";
  if (!validSession(evt.session_id)) return "no_session";
  if (require("path").basename(tp) !== evt.session_id + ".jsonl") return "not_this_sessions";
  return "";
}

function statePath(dir, sid) {
  return require("path").join(dir, sid + ".json");
}

function loadState(dir, sid) {
  try {
    const st = JSON.parse(fs.readFileSync(statePath(dir, sid), "utf8"));
    return st && typeof st === "object" ? st : {};
  } catch (_) {
    return {};
  }
}

function saveState(dir, sid, st) {
  try {
    fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
    const clean = {};
    for (const k of ["off", "ino", "reset", "align"]) if (st[k] !== undefined) clean[k] = st[k];
    fs.writeFileSync(statePath(dir, sid), JSON.stringify(clean), { mode: 0o600 });
  } catch (_) {}
}

// A tool's input, reduced to what its one-line summary needs.
function toolInput(name, input) {
  const i = input && typeof input === "object" ? input : {};
  const out = {};
  const str = (k, n) => {
    if (typeof i[k] === "string") out[k] = clipText(i[k], n || FIELD_MAX);
  };
  const num = (k) => {
    if (typeof i[k] === "number") out[k] = i[k];
  };
  const lines = (s) => (typeof s === "string" ? s.split("\n").length : undefined);
  if (typeof name === "string" && name.startsWith("mcp__")) return out;
  str("command", COMMAND_MAX);
  str("file_path");
  str("notebook_path");
  str("path");
  str("pattern");
  str("description");
  str("subagent_type");
  num("offset");
  num("limit");
  if (name === "Edit") {
    out.lines_old = lines(i.old_string);
    out.lines_new = lines(i.new_string);
  }
  if (name === "Write") out.lines = lines(i.content);
  if (Array.isArray(i.edits)) out.edits_count = i.edits.length;
  if (Array.isArray(i.questions) && i.questions[0] && typeof i.questions[0].question === "string") {
    out.questions = [{ question: clipText(i.questions[0].question, FIELD_MAX) }];
  }
  return out;
}

// A tool result's content as text (images left out) and whether one was there.
function resultText(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((b) => (b && b.type === "text" && typeof b.text === "string" ? b.text : b && b.type === "image" ? "[image]" : ""))
    .join("\n");
}

// One transcript entry as items. Unknown entries and blocks: none.
function entryItems(e) {
  const out = [];
  if (!e || typeof e !== "object" || e.isSidechain) return out;
  const at = Date.parse(e.timestamp) || 0;
  const msg = e.message && typeof e.message === "object" ? e.message : {};
  const content = msg.content;
  if (e.type === "user") {
    if (e.isCompactSummary) {
      out.push({ t: "user", compact_summary: true, text: clipText(resultText(content), TEXT_MAX), at });
      return out;
    }
    const meta = !!e.isMeta;
    if (typeof content === "string") {
      out.push({ t: "user", text: clipText(content, TEXT_MAX), meta, at });
      return out;
    }
    for (const b of Array.isArray(content) ? content : []) {
      if (!b || typeof b !== "object") continue;
      if (b.type === "text") out.push({ t: "user", text: clipText(b.text, TEXT_MAX), meta, at });
      else if (b.type === "image") out.push({ t: "image", at });
      else if (b.type === "tool_result") {
        const text = resultText(b.content);
        out.push({ t: "tool_result", id: clipText(b.tool_use_id, 80), is_error: b.is_error === true, size: text.length, head: clipText(text, HEAD_MAX), at });
      }
    }
  } else if (e.type === "assistant") {
    if (e.isApiErrorMessage) {
      out.push({ t: "api_error", text: clipText(resultText(content), 300), at });
      return out;
    }
    for (const b of Array.isArray(content) ? content : []) {
      if (!b || typeof b !== "object") continue;
      if (b.type === "text") out.push({ t: "assistant", text: clipText(b.text, TEXT_MAX), at });
      else if (b.type === "tool_use") out.push({ t: "tool_use", id: clipText(b.id, 80), name: clipText(b.name, 120), input: toolInput(b.name, b.input), at });
      else if (b.type === "image") out.push({ t: "image", at });
      // thinking, redacted_thinking and anything new: left out.
    }
  } else if (e.type === "system" && e.subtype === "compact_boundary") {
    out.push({ t: "compact", at: Date.parse(e.timestamp) || 0 });
  }
  return out;
}

// What a line too long to read whole says about itself, from its head.
function oversizedItems(head) {
  if (!/"type"\s*:\s*"user"/.test(head) || !/"tool_result"/.test(head)) return [];
  const id = /"tool_use_id"\s*:\s*"([^"]{1,80})"/.exec(head);
  return [{ t: "tool_result", id: id ? id[1] : "", is_error: /"is_error"\s*:\s*true/.test(head), size: 0, head: "" , oversized: true }];
}

// The next piece of the transcript to post: `{from, to, reset, items}`, or
// null when there is nothing new (or the file cannot be read). `st` is this
// session's offset state; `limits` overrides the constants (tests).
function readTail(tp, st, sid, limits) {
  const L = Object.assign({ read: READ_MAX, backfill: BACKFILL_MAX, scan: SCAN_MAX, items: POST_ITEMS, bytes: POST_BYTES }, limits || {});
  let fd;
  try {
    fd = fs.openSync(tp, "r");
    const stat = fs.fstatSync(fd);
    let reset = !!st.reset;
    let align = !!st.align;
    let off = typeof st.off === "number" && st.off >= 0 ? st.off : null;
    // Replaced or truncated: start again from its tail.
    if (off !== null && ((st.ino && st.ino !== stat.ino) || stat.size < off)) {
      off = null;
      reset = true;
    }
    if (off === null) {
      off = Math.max(0, stat.size - L.backfill);
      align = off > 0;
      if (off > 0) reset = true;
    }
    const from = off;
    if (off >= stat.size) return { from, to: off, reset, items: [], ino: stat.ino, prior: null, size: stat.size };
    const len = Math.min(stat.size - off, L.read);
    const buf = Buffer.alloc(len);
    const n = fs.readSync(fd, buf, 0, len, off);
    let start = 0;
    if (align) {
      const nl = buf.indexOf(10);
      if (nl < 0 || nl >= n) {
        // Still inside the line we landed in: skip what was read.
        return { from, to: off + n, reset, items: [], ino: stat.ino, prior: null, align: true, size: stat.size };
      }
      start = nl + 1;
    }
    const last = buf.lastIndexOf(10, n - 1);
    if (last < start) {
      if (n < L.read) return null; // a line still being written
      // One line longer than a read: report it from its head, skip it.
      const items = oversizedItems(buf.slice(start, n).toString("utf8"));
      let pos = off + n;
      const chunk = Buffer.alloc(64 * 1024);
      while (pos - off < L.scan) {
        const m = fs.readSync(fd, chunk, 0, chunk.length, pos);
        if (m <= 0) return null; // not finished being written
        const nl = chunk.indexOf(10);
        if (nl >= 0 && nl < m) return { from, to: pos + nl + 1, reset, items, ino: stat.ino, prior: null, size: stat.size };
        pos += m;
      }
      return { from, to: pos, reset, items, ino: stat.ino, prior: null, align: true, size: Math.max(stat.size, pos + 1) };
    }
    const items = [];
    let prior = null;
    let to = off + start;
    let bytes = 0;
    let p = start;
    while (p <= last) {
      const nl = buf.indexOf(10, p);
      const line = buf.slice(p, nl).toString("utf8");
      p = nl + 1;
      let entry = null;
      try {
        entry = JSON.parse(line);
      } catch (_) {}
      const got = entry ? entryItems(entry) : [];
      if (entry && typeof entry.sessionId === "string" && entry.sessionId !== sid && !prior && validSession(entry.sessionId)) prior = entry.sessionId;
      const size = got.reduce((a, it) => a + JSON.stringify(it).length, 0);
      if (items.length > 0 && (items.length + got.length > L.items || bytes + size > L.bytes)) break; // the rest next time
      items.push(...got.slice(0, L.items));
      bytes += size;
      to = off + p;
    }
    return { from, to, reset, items, ino: stat.ino, prior, size: stat.size };
  } catch (_) {
    return null;
  } finally {
    if (fd !== undefined) {
      try {
        fs.closeSync(fd);
      } catch (_) {}
    }
  }
}

function readable(p) {
  try {
    fs.accessSync(p, fs.constants.R_OK);
    return true;
  } catch (_) {
    return false;
  }
}

// What to keep after the daemon answered a post: its status and `next`.
function stateAfter(status, body, piece) {
  const next = body && typeof body.next === "number" ? body.next : null;
  if (status === 200 && next !== null) return { off: next, ino: piece.ino, align: piece.align ? true : undefined };
  if (status === 409 && next === 0) return { reset: true };
  if (status === 409 && next !== null) return { off: next, ino: piece.ino };
  return null; // unchanged: tried again at the next event
}

// Post what the transcript has that the daemon lacks, piece after piece,
// within one event's budget: at most SYNC_ROUNDS posts and SYNC_BUDGET_MS,
// whichever comes first; the rest goes at the next event. A caught-up hook
// still posts one empty piece, so a daemon that restarted (and lost every
// log) says `409 {next: 0}` now, and the backfill follows in the same run
// rather than when the session next writes. `post(body, then(status,
// parsed))` sends one piece (status 0: no answer); `done({posts, items,
// status})` is called once, at the latest when the budget runs out, whatever
// the daemon does: the hook never waits longer than that for it.
const SYNC_ROUNDS = 12; // a 2 MB backfill in 256 KB reads, and a 409 or two
const SYNC_BUDGET_MS = 1000; // under the hook's 2 s safety net
function syncHistory(tp, sid, dir, post, opts, done) {
  const o = opts || {};
  const rounds = o.rounds || SYNC_ROUNDS;
  let posts = 0;
  let items = 0;
  let status = 0;
  let finished = false;
  const finish = () => {
    if (finished) return;
    finished = true;
    clearTimeout(timer);
    done({ posts, items, status });
  };
  const timer = setTimeout(finish, o.budgetMs || SYNC_BUDGET_MS);
  let st = loadState(dir, sid);
  const step = () => {
    if (finished) return;
    if (posts >= rounds) return finish();
    const piece = readTail(tp, st, sid, o.limits);
    // Nothing to read (a line still being written, an unreadable file), or
    // nothing new after the first post: done for this event.
    if (!piece || (posts > 0 && !(piece.to > piece.from || piece.reset))) return finish();
    const body = { session_id: sid, from: piece.from, to: piece.to, reset: !!piece.reset, items: piece.items };
    if (piece.prior) body.prior_session = piece.prior;
    posts += 1;
    post(body, (s, parsed) => {
      if (finished) return; // too late: the same piece goes again next time
      status = s;
      const next = stateAfter(s, parsed, piece);
      if (!next) return finish();
      saveState(dir, sid, next);
      st = next;
      if (s === 200) {
        items += piece.items.length;
        // Caught up: nothing more this time.
        if (!(piece.to < piece.size)) return finish();
      }
      step();
    });
  };
  step();
}
// END history

// Safety net: never hang the agent if stdin doesn't close.
setTimeout(done, 2000);

let raw = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (c) => (raw += c));
process.stdin.on("end", () => {
  let evt;
  try {
    evt = JSON.parse(raw);
  } catch (_) {
    evt = { raw };
  }
  // The sandbox's own name. os.hostname() agrees today, but only because sbx
  // happens to set it, so it is the last resort.
  evt.sandbox = process.env.SANDBOX_NAME || os.hostname();
  // Who started this session (see `ancestry`). Sent raw rather than classified
  // here: the daemon owns the rule, so it can be corrected without reinstalling
  // the hook into every existing sandbox.
  evt.hook_version = HOOK_VERSION;
  evt.ssh_env = sshEnvNames();
  const chain = ancestors();
  evt.ancestry = chain.map((a) => a.name);
  // The thread sbxw started this session as, if any. Its worktree says the
  // same for a split plan, but a shared plan's threads all share one.
  if (process.env.SBXW_THREAD) evt.sbxw_thread = process.env.SBXW_THREAD;
  // Sessions sbxw itself started in the background — writing a code map or a
  // lens on it — say so, and the daemon files their events nowhere. Nobody attached
  // them, no pane shows them, and there is no terminal in which their prompts
  // could be answered; folded into the sandbox's session state they would light
  // its row as an agent at work and, worse, raise summonses the island offers
  // to answer on a PTY that is not theirs.
  //
  // Keyed on the reporting URL the daemon puts in that session's environment
  // (see `sbx::codemap_run_args`), which is inherited by this hook and set for
  // no other session — including one where a human typed /codemap themselves,
  // which is an ordinary session in every way that matters here.
  if (process.env.SBXW_CODEMAP_DONE) evt.background = "codemap";
  // Claude Code's background sessions (and the spares it keeps ready) run
  // under its own daemon, with no terminal: the daemon lists them as
  // background, never as the terminal's session.
  const kind = sessionKind(process.env);
  if (kind) evt.session_kind = kind;
  // The agent process, and on a start which agent processes are alive, so
  // the daemon can tell a /clear or a killed terminal from a second one.
  const agent = agentProcess(chain);
  if (agent) {
    evt.agent_proc = agent.pid + ":" + agent.start;
    if (evt.hook_event_name === "SessionStart") {
      const live = liveProcs(agent.name);
      if (live) {
        evt.live_procs = live;
        evt.live_scope = "claude";
      }
    }
  }
  // The transcript's tail, unless this is a background session.
  const problem = transcriptProblem(evt);
  let syncing = false;
  if (!evt.background) {
    if (problem) evt.history = "none:" + problem;
    else {
      // Not written yet (a session's first prompt) is catching up, not a
      // failure: the daemon must not fill the log from events meanwhile.
      evt.history = !fs.existsSync(evt.transcript_path) || readable(evt.transcript_path) ? "transcript" : "none:unreadable";
      syncing = evt.history === "transcript";
    }
  }
  log(
    "fire event=" + (evt.hook_event_name || "?") +
      " ssh=" + (evt.ssh_env.join(",") || "-") +
      " ancestry=" + evt.ancestry.join("<") +
      " → host.docker.internal:" + PORT,
  );

  // Both requests at once; the hook exits when both are answered.
  let pending = 1 + (syncing ? 1 : 0);
  send("/api/hook", evt, (status, body) => {
    if (status >= 400) log("status=" + status + " body=" + body.replace(/\s+/g, " ").trim());
    else {
      log("ok status=" + status);
      let out = null;
      try {
        out = JSON.parse(body);
      } catch (_) {}
      if (out && out.hookSpecificOutput) output = out;
    }
    settle();
  });
  if (syncing) {
    const post = (body, then) =>
      send("/api/hook/history", body, (status, text) => {
        let parsed = null;
        try {
          parsed = JSON.parse(text);
        } catch (_) {}
        then(status, parsed);
      });
    syncHistory(evt.transcript_path, evt.session_id, HISTORY_DIR, post, null, (info) => {
      log("history posts=" + info.posts + " items=" + info.items + " status=" + info.status);
      settle();
    });
  }
});

// POST `body` to the daemon; `then(status, text)` once, 0 on a failure.
function send(path, body, then) {
  let called = false;
  const once = (status, text) => {
    if (!called) {
      called = true;
      then(status, text || "");
    }
  };
  const payload = Buffer.from(JSON.stringify(body));
  const req = http.request(
    {
      host: "host.docker.internal",
      port: PORT,
      path,
      method: "POST",
      headers: {
        "content-type": "application/json",
        "content-length": payload.length,
        authorization: "Bearer " + agentToken(),
      },
      timeout: 1500,
    },
    (res) => {
      // On a policy block (403) the proxy returns a structured body telling
      // us exactly which `sbx policy allow` rule is missing: it is logged.
      let text = "";
      res.setEncoding("utf8");
      res.on("data", (c) => (text += c));
      res.on("end", () => once(res.statusCode || 0, text));
      res.on("error", () => once(0, ""));
    },
  );
  req.on("error", (e) => {
    log(path + " error " + (e && e.message));
    once(0, "");
  });
  req.on("timeout", () => {
    log(path + " timeout");
    req.destroy();
    once(0, "");
  });
  req.write(payload);
  req.end();
}
