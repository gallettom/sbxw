// sbxw relay — the one way an agent in a sandbox can ask an agent in *another*
// sandbox for information, with a human deciding every hop.
//
// Nothing here talks to another sandbox. It talks to the sbxw daemon on the
// host, which shows the request in the browser UI and waits for a person to
// route it, then to release the answer. A sandbox therefore cannot pick its
// correspondent, cannot see who answered unless the human sends it, and cannot
// receive a word that was not approved — the network policy alone would not buy
// that, since it only decides *whether* a host is reachable, not what may cross.
//
// Three verbs, all through `http://host.docker.internal:__PORT__`:
//
//   node relay.js ask "question…" [--timeout 90]
//   node relay.js wait <id> [--timeout 90]
//   node relay.js reply <id> "answer…" | --stdin
//
// `ask` and `wait` return as soon as the request settles, and otherwise after
// `--timeout` seconds with the request still open — a bounded call rather than
// an agent's tool blocking for as long as a human takes to look. The id it
// prints is how the same question is picked up again later with `wait`.
//
// Also the transport for the MCP server next door (`relay-mcp.js`), which
// `require`s this file for `request`/`reportOutcome` — everything below runs
// only when this file is the program being executed.
const http = require("http");
const os = require("os");

const PORT = __PORT__;
const HOST = "host.docker.internal";

/// Default seconds a single `ask`/`wait` call blocks before reporting back that
/// the request is still open. Short enough to sit inside an agent's tool
/// timeout, long enough that a human who is already looking usually settles it
/// within the first call.
const DEFAULT_TIMEOUT = 90;

/// Hard ceiling, so a `--timeout` typo cannot wedge a tool call for an hour.
const MAX_TIMEOUT = 600;

function usage(msg) {
  if (msg) process.stderr.write("sbxw relay: " + msg + "\n");
  process.stderr.write(
    "usage:\n" +
      "  node relay.js ask <question> [--timeout <seconds>]\n" +
      "  node relay.js wait <request-id> [--timeout <seconds>]\n" +
      "  node relay.js reply <request-id> (<answer> | --stdin)\n",
  );
  process.exit(1);
}

/// Pull `--timeout <n>` out of `args`, leaving the positional arguments behind.
function takeTimeout(args) {
  const i = args.indexOf("--timeout");
  if (i < 0) return DEFAULT_TIMEOUT;
  const raw = parseInt(args[i + 1], 10);
  args.splice(i, 2);
  if (!Number.isFinite(raw) || raw <= 0) usage("--timeout wants a positive number of seconds");
  return Math.min(raw, MAX_TIMEOUT);
}

function request(path, body, timeoutMs) {
  return new Promise((resolve, reject) => {
    const payload = Buffer.from(JSON.stringify(body));
    const req = http.request(
      {
        host: HOST,
        port: PORT,
        path,
        method: "POST",
        headers: { "content-type": "application/json", "content-length": payload.length },
        // The socket must outlive the *server's* long poll, which is bounded by
        // the `timeout` in the body. Anything tighter would abort a call that
        // was about to answer.
        timeout: timeoutMs,
      },
      (res) => {
        let raw = "";
        res.setEncoding("utf8");
        res.on("data", (c) => (raw += c));
        res.on("end", () => {
          let parsed;
          try {
            parsed = JSON.parse(raw);
          } catch (_) {
            reject(new Error(`unreadable reply from sbxw (HTTP ${res.statusCode}): ${raw.slice(0, 200)}`));
            return;
          }
          if (parsed && parsed.ok === false) {
            reject(new Error(parsed.error || `sbxw refused the call (HTTP ${res.statusCode})`));
            return;
          }
          resolve(parsed);
        });
      },
    );
    req.on("error", (e) =>
      reject(
        new Error(
          `cannot reach the sbxw daemon at ${HOST}:${PORT} (${e && e.message}). ` +
            "It runs on the host as `sbxw web`; this sandbox is allowed to reach it by network policy.",
        ),
      ),
    );
    req.on("timeout", () => {
      req.destroy();
      reject(new Error("the sbxw daemon did not answer in time"));
    });
    req.write(payload);
    req.end();
  });
}

function readStdin() {
  return new Promise((resolve) => {
    let raw = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (c) => (raw += c));
    process.stdin.on("end", () => resolve(raw));
  });
}

const me = process.env.SANDBOX_VM_ID || os.hostname();

/// Describe where a request stands, in the terms the *asking* agent needs: what
/// it may now use, and how to come back if the answer has not been released yet.
function describeOutcome(res, howToWait) {
  const id = res.id;
  // How to come back to an unsettled request. Differs by caller: a shell agent
  // re-runs this CLI, an MCP client calls the tool next door, and telling
  // either one to do the other's thing is how a request gets abandoned.
  const again = howToWait || `run \`node ~/.sbxw/relay.js wait ${id}\``;
  switch (res.state) {
    case "approved":
      // With no `to`, nobody was asked: the human answered it themselves, and
      // saying "sandbox null" would be worse than saying nothing.
      return (
        `Answer to request ${id}, released by the human` +
        (res.to ? ` and written by sandbox "${res.to}"` : "") +
        ":\n\n" +
        res.answer +
        "\n"
      );
    case "denied":
      return (
        `Request ${id} was declined by the human${res.note ? ": " + res.note : "."}\n` +
        "Do not re-send it. Carry on without this information, or ask the human directly.\n"
      );
    case "answered":
      return (
        `Request ${id}: sandbox "${res.to}" has answered and the human is reviewing it.\n` +
        `Nothing has been released to you yet — ${again} to pick it up.\n`
      );
    case "routed":
      return (
        `Request ${id} was sent to sandbox "${res.to}", which has not answered yet.\n` +
        `${again[0].toUpperCase()}${again.slice(1)} to keep waiting.\n`
      );
    default:
      return (
        `Request ${id} is waiting for the human to route it to a sandbox.\n` +
        `${again[0].toUpperCase()}${again.slice(1)} to keep waiting.\n`
      );
  }
}

async function main() {
  const args = process.argv.slice(2);
  const verb = args.shift();
  if (!verb) usage();

  if (verb === "ask") {
    const timeout = takeTimeout(args);
    const question = args.join(" ").trim();
    if (!question) usage("nothing to ask");
    const res = await request(
      "/api/relay/ask",
      { from: me, question, timeout },
      (timeout + 10) * 1000,
    );
    process.stdout.write(describeOutcome(res));
    return;
  }

  if (verb === "wait") {
    const timeout = takeTimeout(args);
    const id = (args.shift() || "").trim();
    if (!id) usage("which request?");
    const res = await request("/api/relay/wait", { from: me, id, timeout }, (timeout + 10) * 1000);
    process.stdout.write(describeOutcome(res));
    return;
  }

  if (verb === "reply") {
    const useStdin = args.includes("--stdin");
    if (useStdin) args.splice(args.indexOf("--stdin"), 1);
    const id = (args.shift() || "").trim();
    if (!id) usage("which request?");
    const answer = (useStdin ? await readStdin() : args.join(" ")).trim();
    if (!answer) usage("nothing to reply");
    await request("/api/relay/reply", { from: me, id, answer }, 15000);
    process.stdout.write(
      `Answer filed for request ${id}. A human reviews it before the other sandbox sees it —\n` +
        "there is nothing further to do here.\n",
    );
    return;
  }

  usage(`unknown verb "${verb}"`);
}

// Only when run as a program. Required as a module — which is what the MCP
// server next door does — this file is just the three helpers below.
if (require.main === module) {
  main().catch((e) => {
    process.stderr.write("sbxw relay: " + (e && e.message ? e.message : String(e)) + "\n");
    process.exit(1);
  });
}

module.exports = { request, describeOutcome, me, DEFAULT_TIMEOUT, MAX_TIMEOUT };
