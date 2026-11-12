#!/usr/bin/env node
// PreToolUse (Bash): steer a page check to the sandbox's shared browser.
//
// The person watches the agent in sbxw's Browser panel, which shows the one
// Chromium that ~/.sbxw/browser/browser.mjs runs. A Chromium the agent
// launches for itself (a one-off screenshot, `chromium.launch()` in a
// throwaway script) is invisible to them, and the agent context saying so
// was not enough: agents fall back on the habit. So such a command is
// refused once, with what to run instead; the agent then uses the shared
// one, or says why it needs its own and prefixes SBXW_OWN_BROWSER=1.
//
// Narrow on purpose: only the command line, and scripts under /tmp (where
// agents keep throwaway ones), are read. A project's own test suite
// (`npx playwright test`, a spec in the repo) is never touched, nor are the
// kits' tools that launch Chromium for their own work.
// Never blocks on its own failure: anything unexpected exits 0.
import { existsSync, readFileSync, statSync } from "node:fs";

const LAUNCH = /\b(chromium|firefox|webkit)\s*\.\s*launch(Persistent(Context)?)?\s*\(|\blaunch_persistent_context\s*\(|\bp\.(chromium|firefox|webkit)\.launch\s*\(/;
const SHARED = /connectOverCDP|connect_over_cdp|127\.0\.0\.1:9222|localhost:9222|browser\.mjs/;
const ONE_SHOT = /\bplaywright(@[\w.^~-]+)?\s+(screenshot|pdf|open|codegen)\b|\b(headless_shell|chrome|chromium|chromium-browser|google-chrome)\b[^\n|;&]*--(headless|screenshot)\b/;

function refuse() {
  process.stderr.write(
    "This launches a browser of your own, which the person cannot see. Use the sandbox's shared browser, " +
      "which they watch live in sbxw's Browser panel:\n" +
      "  node /home/agent/.sbxw/browser/browser.mjs open <url>   (then: text, click <sel>, type <sel> <text>, shot <file.png>)\n" +
      "or, from Playwright, after `node /home/agent/.sbxw/browser/browser.mjs start`:\n" +
      "  chromium.connectOverCDP('http://127.0.0.1:9222')   (Python: p.chromium.connect_over_cdp(...))\n" +
      "Local servers work from it (http://localhost:<port>). If you really need a separate browser " +
      "(a test suite, another engine), say why and prefix the command with SBXW_OWN_BROWSER=1.\n",
  );
  process.exit(2);
}

try {
  const input = JSON.parse(readFileSync(0, "utf8"));
  const cmd = String(input?.tool_input?.command || "");
  if (!cmd || /\bSBXW_OWN_BROWSER=1\b/.test(cmd) || SHARED.test(cmd)) process.exit(0);
  if (ONE_SHOT.test(cmd) || LAUNCH.test(cmd)) refuse();
  // A throwaway script the command runs.
  for (const m of cmd.matchAll(/(\/tmp\/[^\s'";|&()<>]+\.(?:m?js|cjs|ts|py))/g)) {
    const file = m[1];
    if (!existsSync(file) || statSync(file).size > 512 * 1024) continue;
    const text = readFileSync(file, "utf8");
    if (LAUNCH.test(text) && !SHARED.test(text)) refuse();
  }
} catch {
  process.exit(0);
}
