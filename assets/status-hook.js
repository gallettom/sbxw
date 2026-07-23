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
  log("fire event=" + (evt.hook_event_name || "?") + " → host.docker.internal:" + PORT);

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
