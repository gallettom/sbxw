// sbxw usage statusLine — Claude Code invokes this with a JSON payload on stdin
// (see code.claude.com/docs/en/statusline) and renders our stdout as the status
// line. We print a compact line AND (throttled) forward the subscription rate
// limits to the sbxw daemon so the macOS island can show them.
//
// Claude Code fetches these numbers itself (that's the statusLine contract) — we
// only read the structured JSON it hands us. No OAuth token is reused out-of-band.
// The daemon port is templated in at install time (see sbx::install_usage_statusline).
const http = require("http");
const fs = require("fs");
const os = require("os");

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
// statusLine fires on every render; only forward to the daemon this often.
const THROTTLE_MS = 10000;
const STAMP = "/tmp/sbxw-usage-last";

// Safety net: never hang Claude Code's status bar if stdin doesn't close.
setTimeout(() => process.exit(0), 2500);

let raw = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (c) => (raw += c));
process.stdin.on("end", () => {
  let d = {};
  try {
    d = JSON.parse(raw);
  } catch (_) {}

  const model = (d.model && (d.model.display_name || d.model.id)) || "claude";
  const cost =
    d.cost && typeof d.cost.total_cost_usd === "number"
      ? "$" + d.cost.total_cost_usd.toFixed(2)
      : null;
  const rl = d.rate_limits || {};
  const pct = (w) =>
    w && typeof w.used_percentage === "number" ? Math.round(w.used_percentage) : null;
  const fh = pct(rl.five_hour);
  const sd = pct(rl.seven_day);

  // 1) Always print a compact status line — this is what Claude Code shows.
  const parts = [model];
  if (cost) parts.push(cost);
  if (fh !== null) parts.push("5h " + fh + "%");
  if (sd !== null) parts.push("7d " + sd + "%");
  process.stdout.write(parts.join("  ·  "));

  // 1b) The statusLine this one replaced (sbx's own, which feeds Docker's
  // desktop app and prints nothing) gets the same payload. Its output is not
  // ours to show; its failure is not ours to report.
  try {
    const chained = fs.readFileSync(require("path").join(os.homedir(), ".sbxw", "usage-statusline.chain"), "utf8").trim();
    // Synchronous, bounded: the exits below must not cut its stdin short.
    if (chained) {
      require("child_process").spawnSync("sh", ["-c", chained], { input: raw, stdio: ["pipe", "ignore", "ignore"], timeout: 1000 });
    }
  } catch (_) {}

  // 2) Throttled, fire-and-forget forward to the daemon (no output, never blocks).
  // The model this session runs goes with it, and a session whose model is not
  // the one it last sent (the first render, or a `/model` switch) is sent at
  // once, whatever the throttle says. Remembered per session, in /tmp.
  const sessionId = typeof d.session_id === "string" ? d.session_id : "";
  const modelId = (d.model && typeof d.model.id === "string" && d.model.id) || "";
  const modelName = (d.model && typeof d.model.display_name === "string" && d.model.display_name) || "";
  const modelStamp = "/tmp/sbxw-model-" + sessionId.replace(/[^A-Za-z0-9_-]/g, "");
  let sentModel = "";
  try {
    sentModel = fs.readFileSync(modelStamp, "utf8");
  } catch (_) {}
  const modelChanged = modelId !== "" && sentModel !== modelId + "\n" + modelName;
  let last = 0;
  try {
    last = parseInt(fs.readFileSync(STAMP, "utf8"), 10) || 0;
  } catch (_) {}
  if (!modelChanged && Date.now() - last < THROTTLE_MS) {
    return; // event loop drains, process exits, stdout flushed
  }
  try {
    fs.writeFileSync(STAMP, String(Date.now()));
  } catch (_) {}

  const costIn = d.cost || {};
  const num = (v) => (typeof v === "number" && isFinite(v) && v >= 0 ? v : undefined);
  const whole = (v) => (num(v) === undefined ? undefined : Math.round(v));
  const payload = Buffer.from(
    JSON.stringify({
      five_hour_pct: fh,
      seven_day_pct: sd,
      five_hour_resets_at: (rl.five_hour && rl.five_hour.resets_at) || null,
      seven_day_resets_at: (rl.seven_day && rl.seven_day.resets_at) || null,
      // Which session reports, and the model it runs (absent: an older daemon
      // ignores them, a newer one keeps what it had).
      session_id: sessionId || undefined,
      model: modelId || undefined,
      model_name: modelName || undefined,
      // The session's own cost so far (Claude Code's estimate, in dollars),
      // its wall-clock time and the lines it changed: whatever its `cost`
      // object carries. Within the throttle: a new cost alone sends nothing.
      cost_usd: num(costIn.total_cost_usd),
      duration_ms: whole(costIn.total_duration_ms),
      lines_added: whole(costIn.total_lines_added),
      lines_removed: whole(costIn.total_lines_removed),
      // See status-hook.js.
      sandbox: process.env.SANDBOX_NAME || os.hostname(),
    }),
  );
  const req = http.request(
    {
      host: "host.docker.internal",
      port: PORT,
      path: "/api/usage",
      method: "POST",
      headers: {
        "content-type": "application/json",
        "content-length": payload.length, authorization: "Bearer " + agentToken(),
      },
      timeout: 1500,
    },
    (res) => {
      res.resume();
      // Remembered only once the daemon has it, so a failed send is retried.
      if (modelChanged && res.statusCode === 200) {
        try {
          fs.writeFileSync(modelStamp, modelId + "\n" + modelName);
        } catch (_) {}
      }
      process.exit(0);
    },
  );
  req.on("error", () => process.exit(0));
  req.on("timeout", () => {
    req.destroy();
    process.exit(0);
  });
  req.write(payload);
  req.end();
});
