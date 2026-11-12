#!/usr/bin/env node
// Publish a sandbox port on a host loopback port sbx picks (sbx 0.53).
//
//   node publish.mjs <sandbox-port> [<sandbox-port>…]
//
// The request goes to sbx's own API on the sandbox's proxy
// (`$HTTP_PROXY/_sbx/api/ports`), which the `sbxw:dev-ports` kit is granted
// with `approval: none`: no host port is named, so sbx assigns an ephemeral
// one on loopback and asks nobody. The binding lasts until the sandbox stops.
import http from "node:http";

const ports = process.argv.slice(2).map(Number);
if (!ports.length || ports.some((p) => !Number.isInteger(p) || p < 1 || p > 65535)) {
  console.error("usage: node publish.mjs <sandbox-port> [<sandbox-port>…]");
  process.exit(2);
}
const proxy = process.env.HTTP_PROXY || process.env.http_proxy;
if (!proxy) {
  console.error("HTTP_PROXY is not set: this only works inside an sbx sandbox.");
  process.exit(1);
}
const api = new URL("/_sbx/api/ports", proxy.endsWith("/") ? proxy : proxy + "/");
const body = JSON.stringify(ports.map((sandbox_port) => ({ sandbox_port })));

const req = http.request(
  api,
  { method: "POST", headers: { "content-type": "application/json", "content-length": Buffer.byteLength(body) }, timeout: 60000 },
  (res) => {
    let raw = "";
    res.setEncoding("utf8");
    res.on("data", (c) => (raw += c));
    res.on("end", () => {
      if (res.statusCode < 200 || res.statusCode >= 300) {
        console.error(`sbx refused to publish (HTTP ${res.statusCode}): ${raw.trim().slice(0, 500)}`);
        if (res.statusCode === 403 || res.statusCode === 404) {
          console.error("This needs sbx 0.53 or newer and a sandbox created with the sbxw:dev-ports kit.");
        }
        process.exit(1);
      }
      let rows;
      try {
        const parsed = JSON.parse(raw);
        rows = Array.isArray(parsed) ? parsed : parsed.ports || [parsed];
      } catch (_) {
        rows = [];
      }
      const shown = rows.filter((r) => r && r.host_port);
      if (!shown.length) {
        // The shape is sbx's to change: give the answer as it came.
        console.log(raw.trim());
        return;
      }
      for (const r of shown) {
        const ip = r.host_ip && r.host_ip !== "0.0.0.0" ? r.host_ip : "127.0.0.1";
        const host = ip.includes(":") && !ip.startsWith("[") ? `[${ip}]` : ip;
        console.log(`sandbox port ${r.sandbox_port ?? "?"} → http://${host}:${r.host_port} (on the person's machine, until the sandbox stops)`);
      }
    });
  },
);
req.on("timeout", () => req.destroy(new Error("sbx did not answer in 60 seconds")));
req.on("error", (e) => {
  console.error(`could not reach sbx's API at ${api.origin}: ${e.message}`);
  process.exit(1);
});
req.end(body);
