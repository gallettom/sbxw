#!/usr/bin/env node
// Drives Jarvis's `sbxw-config` MCP server (assets/jarvis/.../config-mcp.js)
// over stdio against a stub of the daemon's agent listener: the tools are
// listed, a proposal reaches the stub with the sandbox's token, the outcome
// reads as the session must repeat it, and what must never leave the sandbox
// (a secret, a write to a bundled kit) does not.
//
//   node scripts/test-config-mcp.mjs
import { spawn } from "node:child_process";
import http from "node:http";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const server = path.join(root, "assets/jarvis/files/home/agent/.sbxw/jarvis/config-mcp.js");

const seen = [];
let reply = () => [200, { id: "p1", state: "applied", risk: "low", results: ["wrote jarvis-notes"] }];
const stub = http.createServer((req, res) => {
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    const body = JSON.parse(raw || "{}");
    seen.push({ url: req.url, auth: req.headers.authorization, body });
    const [status, out] = reply(req.url, body);
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify(out));
  });
});
await new Promise((r) => stub.listen(0, "127.0.0.1", r));
const port = stub.address().port;

const home = fs.mkdtempSync(path.join(os.tmpdir(), "jarvis-mcp-"));
const tokenFile = path.join(home, "token");
fs.writeFileSync(tokenFile, "tok-123\n");

const child = spawn(process.execPath, [server], {
  env: { ...process.env, HOME: home, SBXW_AGENT_HOST: "127.0.0.1", SBXW_AGENT_PORT: String(port), SBXW_TOKEN_FILE: tokenFile },
  stdio: ["pipe", "pipe", "inherit"],
});
let out = "";
const waiting = new Map();
child.stdout.setEncoding("utf8");
child.stdout.on("data", (c) => {
  out += c;
  let nl;
  while ((nl = out.indexOf("\n")) >= 0) {
    const msg = JSON.parse(out.slice(0, nl));
    out = out.slice(nl + 1);
    waiting.get(msg.id)?.(msg);
    waiting.delete(msg.id);
  }
});
let next = 1;
function rpc(method, params) {
  const id = next++;
  child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  return new Promise((resolve, reject) => {
    waiting.set(id, resolve);
    setTimeout(() => reject(new Error(`no answer to ${method}`)), 5000);
  });
}
const call = async (name, args) => (await rpc("tools/call", { name, arguments: args })).result;
const text = (r) => r.content.map((c) => c.text).join("\n");

let failures = 0;
async function test(name, f) {
  try {
    await f();
    console.log("ok   " + name);
  } catch (e) {
    failures++;
    console.log("FAIL " + name + "\n     " + (e && e.stack ? e.stack.split("\n").slice(0, 3).join("\n     ") : e));
  }
}

await test("initialize names the server", async () => {
  const r = await rpc("initialize", { protocolVersion: "2025-06-18" });
  assert.equal(r.result.serverInfo.name, "sbxw-config");
  child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");
});

await test("tools/list lists every tool, each with its rules", async () => {
  const r = await rpc("tools/list", {});
  const names = r.result.tools.map((t) => t.name);
  assert.deepEqual(names, ["sbxw_state", "propose_kit", "remove_kit", "set_kits", "set_setting", "set_voice_word", "allow_network", "propose_mcp_server", "remove_mcp_server", "read_desktop_screen", "read_desktop_docs", "propose_desktop_screen", "remove_desktop_screen", "check_proposal"]);
  for (const t of r.result.tools.filter((t) => !["sbxw_state", "check_proposal", "read_desktop_screen", "read_desktop_docs"].includes(t.name))) {
    assert.match(t.description, /PROPOSAL, not a change/, t.name);
    assert.match(t.description, /Never put a secret/, t.name);
    assert.ok(t.inputSchema.required.includes("summary"), t.name);
  }
  const kit = r.result.tools.find((t) => t.name === "propose_kit").description;
  assert.match(kit, /read-only/);
  assert.match(kit, /created AFTER/);
  assert.match(kit, /sbxw kit add/);
  assert.match(kit, /\.claude\/hooks\//);
  const sets = r.result.tools.find((t) => t.name === "set_kits").description;
  assert.match(sets, /sbxw:ssh-agent.*high risk/s);
  assert.match(sets, /spoken yes does not count/);
  const word = r.result.tools.find((t) => t.name === "set_voice_word").description;
  assert.match(word, /pronounced badly/);
  assert.match(word, /never what dictation writes/);
  const mcp = r.result.tools.find((t) => t.name === "propose_mcp_server").description;
  assert.match(mcp, /Prefer `remote`/);
  assert.match(mcp, /tick an acknowledgement/);
  const screen = r.result.tools.find((t) => t.name === "propose_desktop_screen").description;
  assert.match(screen, /`read_desktop_docs`, then `read_desktop_screen`/);
  assert.match(screen, /Keep changes small/);
  assert.match(screen, /last good view/);
  assert.match(screen, /cannot add a screen/);
  assert.match(screen, /click on the diff/);
  assert.match(r.result.tools.find((t) => t.name === "remove_desktop_screen").description, /never a spoken yes/);
  assert.match(r.result.tools.find((t) => t.name === "read_desktop_docs").description, /BEFORE/);
});

await test("a proposal reaches the daemon with the token and reads back applied", async () => {
  seen.length = 0;
  const r = await call("propose_kit", {
    summary: "Add a release-notes skill to a new jarvis-notes kit",
    kit: "notes",
    description: "Release notes",
    skills: [{ name: "release-notes", content: "---\nname: release-notes\ndescription: x\n---\nWrite notes." }],
    commands: [{ name: "/notes", content: "Run the skill." }],
    files: [{ path: "/home/agent/.config/notes.toml", content: "a = 1\n" }],
    context: "Use /notes for release notes.",
    timeout_seconds: 5,
  });
  assert.equal(seen.length, 1);
  const s = seen[0];
  assert.equal(s.url, "/api/jarvis/propose");
  assert.equal(s.auth, "Bearer tok-123");
  assert.equal(s.body.summary, "Add a release-notes skill to a new jarvis-notes kit");
  assert.equal(s.body.timeout, 5);
  assert.deepEqual(s.body.changes, [
    {
      kind: "kit_write",
      kit: "jarvis-notes",
      files: [
        { path: ".config/notes.toml", content: "a = 1\n" },
        { path: ".claude/skills/release-notes/SKILL.md", content: "---\nname: release-notes\ndescription: x\n---\nWrite notes." },
        { path: ".claude/commands/notes.md", content: "Run the skill." },
      ],
      remove: [],
      description: "Release notes",
      context: "Use /notes for release notes.",
    },
  ]);
  assert.equal(r.isError, false);
  assert.match(text(r), /^Applied: proposal p1/);
  assert.match(text(r), /created AFTER/);
});

await test("every other kind is shaped as the contract says", async () => {
  const cases = [
    ["remove_kit", { kit: "jarvis-notes" }, { kind: "kit_remove", kit: "jarvis-notes" }],
    ["set_kits", { file: "daemon", add: ["sbxw:retro"] }, { kind: "kits_line", file: "daemon", add: ["sbxw:retro"], remove: [] }],
    ["set_setting", { key: "keep_running", value: true }, { kind: "setting", key: "keep_running", value: true }],
    ["set_voice_word", { word: "nginx", fr: " engine ixe ", en: "engine X" }, { kind: "voice_word", word: "nginx", fr: "engine ixe", en: "engine X" }],
    ["set_voice_word", { word: "sbxw", en: "S B X W" }, { kind: "voice_word", word: "sbxw", en: "S B X W" }],
    ["set_voice_word", { word: "sbxw", remove: true }, { kind: "voice_word", word: "sbxw" }],
    ["allow_network", { add: ["pypi.org"] }, { kind: "network_allow", add: ["pypi.org"], remove: [] }],
    ["propose_mcp_server", { spec: { name: "docs", kind: "remote", url: "https://example.org/mcp" } }, { kind: "mcp_server", spec: { name: "docs", kind: "remote", url: "https://example.org/mcp" } }],
    ["remove_mcp_server", { name: "docs" }, { kind: "mcp_remove", name: "docs" }],
    ["propose_desktop_screen", { name: "topbar.luau", content: "return {}\n" }, { kind: "desktop_screen", name: "topbar", content: "return {}\n" }],
    ["remove_desktop_screen", { name: "topbar" }, { kind: "desktop_screen_remove", name: "topbar" }],
  ];
  for (const [tool, args, change] of cases) {
    seen.length = 0;
    await call(tool, { summary: "s", ...args });
    assert.equal(seen.length, 1, tool);
    assert.deepEqual(seen[0].body.changes, [change], tool);
  }
});

await test("pending, then check_proposal waits on the same id", async () => {
  reply = (url) => [200, url === "/api/jarvis/wait" ? { id: "p2", state: "refused", error: "not now" } : { id: "p2", state: "pending", risk: "high" }];
  const r = await call("allow_network", { summary: "Allow pypi", add: ["pypi.org"] });
  assert.match(text(r), /^Still pending: proposal p2 \(risk: high\)/);
  assert.match(text(r), /check_proposal/);
  seen.length = 0;
  const w = await call("check_proposal", { id: "p2", timeout_seconds: 3 });
  assert.deepEqual(seen.map((s) => [s.url, s.body]), [["/api/jarvis/wait", { id: "p2", timeout: 3 }]]);
  assert.match(text(w), /^Refused: proposal p2 was not applied\. Reason: not now/);
});

await test("failed reads as failed, with the error", async () => {
  reply = () => [200, { id: "p3", state: "failed", error: "sbx mcp add: exit 1" }];
  const r = await call("remove_mcp_server", { summary: "s", name: "docs" });
  assert.equal(r.isError, true);
  assert.match(text(r), /^Failed: proposal p3 .*sbx mcp add: exit 1/);
});

await test("a secret, a bundled kit or a missing summary never leaves the sandbox", async () => {
  seen.length = 0;
  const secret = await call("propose_kit", { summary: "s", kit: "x", files: [{ path: "a", content: "key = ghp_" + "a".repeat(36) }] });
  assert.equal(secret.isError, true);
  assert.match(text(secret), /GitHub token/);
  const bundled = await call("propose_kit", { summary: "s", kit: "sbxw:retro", files: [{ path: "a", content: "x" }] });
  assert.match(text(bundled), /read-only/);
  const escape = await call("propose_kit", { summary: "s", kit: "x", files: [{ path: "../../etc/passwd", content: "x" }] });
  assert.match(text(escape), /not a path relative/);
  const none = await call("set_kits", { file: "daemon", add: ["sbxw:retro"] });
  assert.match(text(none), /summary/);
  const unsaid = await call("set_voice_word", { summary: "s", word: "nginx" });
  assert.match(text(unsaid), /Say how the word is said/);
  const both = await call("set_voice_word", { summary: "s", word: "nginx", fr: "x", remove: true });
  assert.match(text(both), /not both/);
  const path = await call("propose_desktop_screen", { summary: "s", name: "../../x", content: "return {}" });
  assert.match(text(path), /not a screen name/);
  const empty = await call("propose_desktop_screen", { summary: "s", name: "topbar", content: " " });
  assert.match(text(empty), /remove_desktop_screen/);
  const big = await call("propose_desktop_screen", { summary: "s", name: "topbar", content: "x".repeat(64 * 1024 + 1) });
  assert.match(text(big), /64 KB/);
  const badRead = await call("read_desktop_screen", { name: "/etc/passwd" });
  assert.equal(badRead.isError, true);
  assert.equal(seen.length, 0);
});

await test("sbxw_state is a read, and a refusal comes back as a failed tool call", async () => {
  reply = (url) => (url === "/api/jarvis/state" ? [200, { kits: [] }] : [403, { ok: false, error: "not the Jarvis sandbox" }]);
  seen.length = 0;
  const st = await call("sbxw_state", {});
  assert.deepEqual(JSON.parse(text(st)), { kits: [] });
  assert.equal(seen[0].url, "/api/jarvis/state");
  reply = (url, body) => [200, url === "/api/jarvis/desktop/screen" ? { ok: true, name: body.name, override: false, content: "c", builtin: "c" } : { ok: true, screens_md: "# Writing screens", intents: [] }];
  seen.length = 0;
  const scr = await call("read_desktop_screen", { name: "topbar" });
  assert.equal(JSON.parse(text(scr)).name, "topbar");
  const docs = await call("read_desktop_docs", {});
  assert.equal(JSON.parse(text(docs)).screens_md, "# Writing screens");
  assert.deepEqual(seen.map((s) => [s.url, s.body]), [["/api/jarvis/desktop/screen", { name: "topbar" }], ["/api/jarvis/desktop/docs", {}]]);
  reply = () => [403, { ok: false, error: "not the Jarvis sandbox" }];
  const r = await call("remove_kit", { summary: "s", kit: "jarvis-x" });
  assert.equal(r.isError, true);
  assert.match(text(r), /only the Jarvis sandbox/);
});

child.stdin.end();
stub.close();
fs.rmSync(home, { recursive: true, force: true });
if (failures) {
  console.log(`${failures} failed`);
  process.exit(1);
}
console.log("all passed");
