// Common ground for the kit's three hooks: what a hook is handed, where this
// repository's map is, and the state file that keeps a nudge from becoming
// nagging.
//
// The hooks share it rather than each carrying its own copy, because "is this
// repo mapped?" has to give all three the same answer — a Stop hook offering to
// write a map a SessionStart hook has just injected is worse than either hook
// alone.
//
// Written without template literals on purpose: sbx kit `content` fields reject
// dollar-brace, which is what a JS template literal is made of.

import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

export const MAP_REL = ".sbxw-artifacts/codemap";
export const LENS_REL = ".sbxw-artifacts/codemap-lenses";
export const CHECKER = "/home/agent/.sbxw/codemap/check-codemap.mjs";

const STATE = "/home/agent/.sbxw/codemap/state.json";
const WEEK = 7 * 24 * 60 * 60 * 1000;

/** Claude Code hands a hook its payload as JSON on stdin.
 *
 * Anything unreadable comes back as an empty object, and the read gives up
 * after five seconds: a hook that throws or hangs on its own input breaks every
 * session in the sandbox, which is a far worse failure than a missed nudge.
 */
export function readInput() {
  return new Promise((done) => {
    let raw = "";
    const finish = (v) => { done(v); };
    setTimeout(() => finish({}), 5000).unref();
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (c) => { raw += c; });
    process.stdin.on("end", () => { try { finish(JSON.parse(raw)); } catch (e) { finish({}); } });
    process.stdin.on("error", () => finish({}));
  });
}

/** The repository the session is working in: the nearest ancestor of `cwd`
 * holding a `.git`, falling back to `cwd` itself. A map belongs to a project,
 * and every path in it is relative to that project's root. */
export function repoRoot(cwd) {
  const start = resolve(cwd || process.cwd());
  let dir = start;
  for (;;) {
    if (existsSync(join(dir, ".git"))) return dir;
    const up = dirname(dir);
    if (up === dir) return start;
    dir = up;
  }
}

const isDir = (p) => { try { return statSync(p).isDirectory(); } catch (e) { return false; } };

/** What this repository already carries, if anything:
 *
 *  - `{ kind: "codemap", index }` — a map written by this kit.
 *  - `{ kind: "lat", index }`     — a real lat.md vault. The kit defers to it:
 *     it is the same format run by the actual tool, and offering to write a
 *     second map beside it is the one thing /codemap is told never to do.
 *
 * `null` is the unmapped case, and the only one the Stop hook acts on.
 */
export function findMap(root) {
  const index = join(root, MAP_REL, "codemap.md");
  if (existsSync(index)) return { kind: "codemap", index: index };
  const lat = join(root, "lat.md");
  if (isDir(lat)) {
    const named = ["lat.md.md", "index.md", "README.md"].map((n) => join(lat, n));
    return { kind: "lat", index: named.find((p) => existsSync(p)) || null };
  }
  return null;
}

/** The map's sections, by file name — what a reader would pick from. */
export function sections(root) {
  try {
    return readdirSync(join(root, MAP_REL))
      .filter((n) => n.endsWith(".md") && n !== "codemap.md")
      .sort();
  } catch (e) { return []; }
}

/** Every lens written beside the map, by directory name. */
export function lenses(root) {
  try {
    return readdirSync(join(root, LENS_REL), { withFileTypes: true })
      .filter((e) => e.isDirectory())
      .map((e) => e.name)
      .sort();
  } catch (e) { return []; }
}

export function clip(text, max) {
  return text.length <= max ? text : text.slice(0, max) + "\n… (clipped — read the file for the rest)";
}

/** How long ago this hook last fired for `key`, in ms; `Infinity` if never. */
export function since(bucket, key) {
  let state = {};
  try { state = JSON.parse(readFileSync(STATE, "utf8")); } catch (e) { /* never fired */ }
  const at = (state[bucket] || {})[key];
  return typeof at === "number" ? Date.now() - at : Infinity;
}

/** Record that it fired now, dropping anything older than a week so the file
 * does not grow a line per session for the life of the sandbox. */
export function stamp(bucket, key, now) {
  let state = {};
  try { state = JSON.parse(readFileSync(STATE, "utf8")); } catch (e) { /* first fire */ }
  const b = state[bucket] = state[bucket] || {};
  for (const k of Object.keys(b)) if (now - b[k] > WEEK) delete b[k];
  b[key] = now;
  try {
    mkdirSync(dirname(STATE), { recursive: true });
    writeFileSync(STATE, JSON.stringify(state));
  } catch (e) { /* a nudge is not worth failing a session over */ }
}

/** sbxw's own background codemap sessions run with this set (see
 * `sbx::codemap_run_args`). They are writing the map: nudging them to read one
 * is noise, and offering them one is a loop. */
export const isBackgroundRun = () => Boolean(process.env.SBXW_CODEMAP_DONE);

/** The off switch, for a sandbox that wants the commands without the prompting:
 * `echo "export SBXW_CODEMAP_NUDGE=0" >> /etc/sandbox-persistent.sh`. */
export const nudgingOff = () => process.env.SBXW_CODEMAP_NUDGE === "0";
