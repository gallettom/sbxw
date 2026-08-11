// sbxw status hook — forwards Claude Code hook events to the sbxw daemon on the
// host so the island can show *trusted, structured* session state instead of
// scraping the terminal.
//
// It is fire-and-forget: it never blocks Claude Code, never influences a tool
// decision, and always exits 0 with no stdout (so "no decision, normal flow").
// The daemon port is templated in at install time (see sbx::install_status_hooks).
const http = require("http");
const os = require("os");
const fs = require("fs");

const PORT = __PORT__;

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
const HOOK_VERSION = 2;

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
// throughout: an unreadable chain yields an empty list.
function ancestry() {
  const names = [];
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
    names.push(stat.slice(open + 1, close));
    // After ")" comes " <state> <ppid> …".
    const after = stat.slice(close + 1).trim().split(/\s+/);
    const ppid = parseInt(after[1], 10);
    if (!Number.isFinite(ppid) || ppid <= 0) break;
    pid = ppid;
  }
  return names;
}
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
  evt.sandbox = process.env.SANDBOX_VM_ID || os.hostname();
  // Who started this session (see `ancestry`). Sent raw rather than classified
  // here: the daemon owns the rule, so it can be corrected without reinstalling
  // the hook into every existing sandbox.
  evt.hook_version = HOOK_VERSION;
  evt.ssh_env = sshEnvNames();
  evt.ancestry = ancestry();
  log(
    "fire event=" + (evt.hook_event_name || "?") +
      " ssh=" + (evt.ssh_env.join(",") || "-") +
      " ancestry=" + evt.ancestry.join("<") +
      " → host.docker.internal:" + PORT,
  );

  const payload = Buffer.from(JSON.stringify(evt));
  const req = http.request(
    {
      host: "host.docker.internal",
      port: PORT,
      path: "/api/hook",
      method: "POST",
      headers: {
        "content-type": "application/json",
        "content-length": payload.length,
      },
      timeout: 1500,
    },
    (res) => {
      // On a policy block (403) the proxy returns a structured body telling us
      // exactly which `sbx policy allow` rule is missing — capture it.
      if (res.statusCode && res.statusCode >= 400) {
        let body = "";
        res.setEncoding("utf8");
        res.on("data", (c) => (body += c));
        res.on("end", () => {
          log("status=" + res.statusCode + " body=" + body.replace(/\s+/g, " ").trim());
          done();
        });
        return;
      }
      res.resume();
      log("ok status=" + res.statusCode);
      done();
    },
  );
  req.on("error", (e) => {
    log("error " + (e && e.message));
    done();
  });
  req.on("timeout", () => {
    log("timeout");
    req.destroy();
    done();
  });
  req.write(payload);
  req.end();
});
