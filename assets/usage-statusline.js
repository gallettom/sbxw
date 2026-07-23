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

  // 2) Throttled, fire-and-forget forward to the daemon (no output, never blocks).
  let last = 0;
  try {
    last = parseInt(fs.readFileSync(STAMP, "utf8"), 10) || 0;
  } catch (_) {}
  if (Date.now() - last < THROTTLE_MS) {
    return; // event loop drains, process exits, stdout flushed
  }
  try {
    fs.writeFileSync(STAMP, String(Date.now()));
  } catch (_) {}

  const payload = Buffer.from(
    JSON.stringify({
      five_hour_pct: fh,
      seven_day_pct: sd,
      five_hour_resets_at: (rl.five_hour && rl.five_hour.resets_at) || null,
      seven_day_resets_at: (rl.seven_day && rl.seven_day.resets_at) || null,
      sandbox: process.env.SANDBOX_VM_ID || os.hostname(),
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
        "content-length": payload.length,
      },
      timeout: 1500,
    },
    (res) => {
      res.resume();
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
