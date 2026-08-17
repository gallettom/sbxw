// sbxw relay, as an MCP server — so asking another sandbox is a *tool the agent
// can see*, not a shell command described in a paragraph it read at startup.
//
// This exists because of an observed failure, not a hunch. With the relay
// available only as `node ~/.sbxw/relay.js ask …` plus a note in the agent's
// memory, a session that had searched its workspace, established that the code
// it needed lived in a repo that wasn't mounted, and was about to tell the user
// so, reached instead for the one thing in its tool list that looked adjacent
// (a codebase search) and offered *that*. The relay never came up. A tool is
// weighed on every turn; a paragraph competes with the whole conversation.
//
// Transport is stdio JSON-RPC 2.0, newline-delimited, per the MCP spec. The
// three methods a client needs are `initialize`, `tools/list` and `tools/call`;
// notifications (no `id`) are acknowledged with silence, as the spec requires.
// No dependencies: this has to run in a sandbox where nothing was installed for
// it, and `npx`-ing a package would need network access the policy denies.
const relay = require("./relay.js");

const SERVER = { name: "sbxw-relay", version: "1" };

/// Echoed back to the client when it asks for something we don't recognise.
/// Pinning a version we were not offered is how a handshake fails on a client
/// newer than this file.
const FALLBACK_PROTOCOL = "2025-06-18";

/// What an unsettled request is picked up with, in this client's terms.
const HOW_TO_WAIT = id => `call \`check_sandbox_question\` with request_id "${id}"`;

// ── Tools ─────────────────────────────────────────────────────────────────
//
// The descriptions below are the load-bearing part of this file. They are
// written as *triggers* — the situation in which to reach for the tool — rather
// than as definitions of what it does, because the failure being fixed is not
// "the agent misunderstood the tool" but "the agent never considered it".

const TOOLS = [
  {
    name: "ask_other_sandbox",
    description:
      "Ask the agent working in ANOTHER sandbox on this machine a question, when the answer " +
      "lies outside the workspace you can see.\n" +
      "\n" +
      "Reach for this the moment a search comes up empty because what you need lives in a " +
      "different repo, project or service that is not mounted here — a schema, an API's real " +
      "response shape, a convention decided elsewhere, how a library you only have a fragment " +
      "of is wired up. Concretely: use it BEFORE telling the user you cannot see something, " +
      "and BEFORE offering to go looking elsewhere. If you are about to write \"this workspace " +
      "only contains…\", \"I can't confirm without…\" or \"would you like me to search…\", ask " +
      "here first and answer the question instead.\n" +
      "\n" +
      "A human sees your question, chooses which sandbox (if any) receives it, reads the reply " +
      "and decides whether you get it — so you never pick the recipient and never see anything " +
      "that was not released to you. That review is also why the question must stand on its " +
      "own: the agent reading it knows nothing of your conversation. Give the project or " +
      "component by name, say what you already established, and ask one specific thing.\n" +
      "\n" +
      "Returns within ~90 s. If nobody has acted by then it says so and gives you a request id " +
      "to pick up later — carry on with what you can do meanwhile rather than blocking. Never " +
      "send secrets, credentials, or file contents you were not asked to share. Do not use it " +
      "for anything readable in your own workspace: it costs a person's attention every time.",
    inputSchema: {
      type: "object",
      properties: {
        question: {
          type: "string",
          description:
            "The question, self-contained. Name the project/component, state what you already " +
            "know, and ask one specific thing. Read by a human first, then by an agent with no " +
            "knowledge of your session.",
        },
        timeout_seconds: {
          type: "number",
          description:
            "How long to wait before reporting back that it is still open. Default 90, max 600.",
        },
      },
      required: ["question"],
    },
  },
  {
    name: "check_sandbox_question",
    description:
      "Pick up a question you asked earlier with `ask_other_sandbox` that had not been settled " +
      "when the call returned. Returns the approved answer, the refusal, or that it is still " +
      "waiting. Use it when you reach the point where you actually need that answer — not in a " +
      "loop.",
    inputSchema: {
      type: "object",
      properties: {
        request_id: { type: "string", description: "The id reported by `ask_other_sandbox`." },
        timeout_seconds: {
          type: "number",
          description: "How long to wait before reporting back. Default 90, max 600.",
        },
      },
      required: ["request_id"],
    },
  },
];

// ── Dispatch ──────────────────────────────────────────────────────────────

function clampTimeout(v) {
  const n = Number(v);
  if (!Number.isFinite(n) || n <= 0) return relay.DEFAULT_TIMEOUT;
  return Math.min(Math.round(n), relay.MAX_TIMEOUT);
}

/// Every tool result is text. An error in *reaching* the daemon is reported as
/// a failed tool call (`isError`), never as a protocol error: the session
/// should carry on without the relay, not fall over because of it.
function textResult(text, isError) {
  return { content: [{ type: "text", text }], isError: !!isError };
}

async function callTool(name, args) {
  const timeout = clampTimeout((args || {}).timeout_seconds);
  const budget = (timeout + 10) * 1000;

  if (name === "ask_other_sandbox") {
    const question = String((args || {}).question || "").trim();
    if (!question) return textResult("No question given.", true);
    const res = await relay.request(
      "/api/relay/ask",
      { from: relay.me, question, timeout },
      budget,
    );
    return textResult(relay.describeOutcome(res, HOW_TO_WAIT(res.id)));
  }

  if (name === "check_sandbox_question") {
    const id = String((args || {}).request_id || "").trim();
    if (!id) return textResult("No request_id given.", true);
    const res = await relay.request(
      "/api/relay/wait",
      { from: relay.me, id, timeout },
      budget,
    );
    return textResult(relay.describeOutcome(res, HOW_TO_WAIT(res.id)));
  }

  return textResult(`Unknown tool "${name}".`, true);
}

async function handle(msg) {
  switch (msg.method) {
    case "initialize":
      return {
        // Speak the version the client opened with; it is the one both sides
        // are known to share.
        protocolVersion: (msg.params && msg.params.protocolVersion) || FALLBACK_PROTOCOL,
        capabilities: { tools: {} },
        serverInfo: SERVER,
      };
    case "ping":
      return {};
    case "tools/list":
      return { tools: TOOLS };
    case "tools/call":
      return await callTool(msg.params && msg.params.name, msg.params && msg.params.arguments);
    default:
      return { __error: { code: -32601, message: `Method not found: ${msg.method}` } };
  }
}

// ── stdio loop ────────────────────────────────────────────────────────────

function send(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

let buffer = "";
// A call can be parked on a human for a minute or more, so closing stdin must
// not take it down with it: the answer it is waiting for is the whole reason
// the process is alive. Exit once nothing is in flight instead.
let inFlight = 0;
let ended = false;
const exitIfDone = () => {
  if (ended && inFlight === 0) process.exit(0);
};

process.stdin.setEncoding("utf8");
process.stdin.on("data", chunk => {
  buffer += chunk;
  let nl;
  while ((nl = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, nl).trim();
    buffer = buffer.slice(nl + 1);
    if (line) dispatch(line);
  }
});
process.stdin.on("end", () => {
  ended = true;
  exitIfDone();
});

async function dispatch(line) {
  inFlight++;
  try {
    await dispatchOne(line);
  } finally {
    inFlight--;
    exitIfDone();
  }
}

async function dispatchOne(line) {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch (_) {
    send({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } });
    return;
  }
  // A notification has no `id` and must not be answered at all.
  const isRequest = msg.id !== undefined && msg.id !== null;
  try {
    const result = await handle(msg);
    if (!isRequest) return;
    if (result && result.__error) send({ jsonrpc: "2.0", id: msg.id, error: result.__error });
    else send({ jsonrpc: "2.0", id: msg.id, result });
  } catch (e) {
    if (!isRequest) return;
    // The daemon being unreachable is a *tool* failure, not a broken server —
    // reported inside a result so the agent reads the reason and moves on.
    send({
      jsonrpc: "2.0",
      id: msg.id,
      result: textResult(`sbxw relay: ${(e && e.message) || e}`, true),
    });
  }
}
