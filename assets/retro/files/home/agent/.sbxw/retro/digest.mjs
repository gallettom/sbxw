#!/usr/bin/env node
// Boil one or more Claude Code session transcripts down to what /retro reads:
// what the person asked, what the agent did, what failed, and what repeated.
//
// A transcript is ~/.claude/projects/<project slug>/<session id>.jsonl, often
// megabytes of request blobs and tool output. The agent cannot read that whole,
// and should not: the patterns /retro looks for (the same command run ten
// times, the same correction given twice, the same file re-read) are counts
// across the whole file, which a script gets right and a skim does not.
//
//   digest.mjs --list              sessions of this project, newest first
//   digest.mjs                     the newest session (usually the current one)
//   digest.mjs <id-prefix|path>…   those sessions
//   digest.mjs --last N            the N newest sessions, with cross-session counts
//   --project <dir>                another project's sessions (default: cwd)
//
// Prints to stdout. Long values are cut, and anything shaped like a token is
// masked, because the digest ends up quoted in a report.

import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { join, basename } from "node:path";
import { homedir } from "node:os";

const args = process.argv.slice(2);
const take = (flag) => {
  const i = args.indexOf(flag);
  if (i < 0) return null;
  const v = args[i + 1];
  args.splice(i, 2);
  return v;
};
const list = args.includes("--list") && !!args.splice(args.indexOf("--list"), 1);
const last = Number(take("--last") || 0);
const project = take("--project") || process.cwd();

const ROOT = join(homedir(), ".claude", "projects");
const dir = join(ROOT, project.replace(/[^a-zA-Z0-9]/g, "-"));

const SECRET = /(sk-[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{20,}|xox[abp]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|eyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+|(?:token|secret|password|api[_-]?key)["'=: ]+[^\s"']{8,})/gi;
const clean = (s, n = 200) => {
  s = String(s ?? "").replace(SECRET, "«masked»").replace(/\s+/g, " ").trim();
  return s.length > n ? s.slice(0, n) + "…" : s;
};

function sessions() {
  if (!existsSync(dir)) {
    console.error(`no sessions for ${project} (looked in ${dir})`);
    process.exit(1);
  }
  return readdirSync(dir)
    .filter((f) => f.endsWith(".jsonl"))
    .map((f) => ({ path: join(dir, f), mtime: statSync(join(dir, f)).mtimeMs }))
    .sort((a, b) => b.mtime - a.mtime);
}

function load(path) {
  const out = [];
  for (const line of readFileSync(path, "utf8").split("\n")) {
    if (!line.trim()) continue;
    try { out.push(JSON.parse(line)); } catch { /* a line cut by a live write */ }
  }
  return out;
}

const textOf = (content) =>
  typeof content === "string" ? content
    : Array.isArray(content) ? content.filter((c) => c.type === "text").map((c) => c.text).join(" ")
    : "";

// What a person typed, as opposed to tool results, hook output and the
// harness's own injected turns, which all travel as `user` entries too.
const isHuman = (e) =>
  e.type === "user" && !e.isMeta && !e.isSidechain &&
  (e.origin?.kind ? e.origin.kind === "human" : typeof e.message?.content === "string") &&
  !/^<(command-|local-command|system-reminder)/.test(textOf(e.message?.content).trim());

function summarize(name, input = {}) {
  switch (name) {
    case "Bash": return clean(input.command, 160);
    case "Read": case "Write": case "Edit": case "NotebookEdit": return clean(input.file_path, 160);
    case "Grep": case "Glob": return clean(`${input.pattern}${input.path ? "  in " + input.path : ""}`, 160);
    case "Skill": return clean(`/${input.skill} ${input.args || ""}`, 160);
    case "Agent": case "Task": return clean(`${input.subagent_type || ""} ${input.description || ""}`, 160);
    case "WebFetch": return clean(input.url, 160);
    case "WebSearch": return clean(input.query, 160);
    default: return clean(JSON.stringify(input), 120);
  }
}

// `npm test -- a.spec.ts` and `npm test -- b.spec.ts` are the same habit.
const shape = (cmd) =>
  String(cmd).replace(/^cd [^&;]+(&&|;)\s*/, "").trim().split(/\s+/).slice(0, 2).join(" ");

function digest(path) {
  const entries = load(path);
  const id = basename(path, ".jsonl");
  const title = entries.filter((e) => e.type === "ai-title").map((e) => e.aiTitle || e.title).pop();
  const stamps = entries.map((e) => e.timestamp).filter(Boolean).sort();
  const cwd = entries.find((e) => e.cwd)?.cwd;

  const prompts = [];
  const calls = [];
  const byId = new Map();
  const errors = [];
  const hooks = new Map();
  let step = 0;

  for (const e of entries) {
    if (isHuman(e)) {
      prompts.push({ step, text: clean(textOf(e.message.content), 500) });
      continue;
    }
    const hook = e.attachment?.hookName || (e.type === "system" && e.hookEvent ? e.hookEvent : null);
    if (hook) hooks.set(hook, (hooks.get(hook) || 0) + 1);
    if (e.type === "assistant" && !e.isSidechain) {
      for (const c of e.message?.content || []) {
        if (c.type !== "tool_use") continue;
        const call = { n: ++step, name: c.name, what: summarize(c.name, c.input), input: c.input || {} };
        calls.push(call);
        byId.set(c.id, call);
      }
    }
    if (e.type === "user" && Array.isArray(e.message?.content)) {
      for (const c of e.message.content) {
        if (c.type !== "tool_result") continue;
        const body = textOf(c.content) || (typeof c.content === "string" ? c.content : "");
        const refused = /doesn't want to proceed|was rejected|Request interrupted/i.test(body);
        if (c.is_error || refused) {
          const call = byId.get(c.tool_use_id);
          errors.push({ n: call?.n ?? "?", name: call?.name ?? "?", refused, text: clean(body, 220) });
        }
      }
    }
  }

  const count = (xs) => [...xs.reduce((m, x) => m.set(x, (m.get(x) || 0) + 1), new Map())].sort((a, b) => b[1] - a[1]);
  const tools = count(calls.map((c) => c.name));
  const bash = count(calls.filter((c) => c.name === "Bash").map((c) => shape(c.input.command)));
  const reads = count(calls.filter((c) => c.name === "Read").map((c) => c.input.file_path));
  const edits = count(calls.filter((c) => ["Edit", "Write"].includes(c.name)).map((c) => c.input.file_path));

  return { id, title, cwd, stamps, prompts, calls, errors, hooks: [...hooks], tools, bash, reads, edits };
}

function print(d) {
  const out = [];
  out.push(`# session ${d.id}${d.title ? " — " + d.title : ""}`);
  out.push(`cwd: ${d.cwd || "?"}   from ${d.stamps[0] || "?"} to ${d.stamps.at(-1) || "?"}`);
  out.push(`${d.prompts.length} prompts, ${d.calls.length} tool calls, ${d.errors.length} errors or refusals`);

  out.push(`\n## What the person typed (step = tool calls before it)`);
  for (const p of d.prompts) out.push(`- [@${p.step}] ${p.text}`);

  out.push(`\n## Tools used`);
  out.push(d.tools.map(([k, v]) => `${k} ×${v}`).join(", ") || "none");

  const rep = (title, rows, min) => {
    const r = rows.filter(([, v]) => v >= min);
    if (!r.length) return;
    out.push(`\n## ${title}`);
    for (const [k, v] of r.slice(0, 15)) out.push(`- ×${v}  ${clean(k, 140)}`);
  };
  rep("Repeated shell commands (by first two words)", d.bash, 3);
  rep("Files read more than once", d.reads, 2);
  rep("Files edited more than twice", d.edits, 3);

  if (d.errors.length) {
    out.push(`\n## Errors and refusals`);
    for (const e of d.errors.slice(0, 30)) out.push(`- #${e.n} ${e.name}${e.refused ? " (refused/interrupted)" : ""}: ${e.text}`);
  }

  if (d.hooks.length) {
    out.push(`\n## Hooks that fired`);
    out.push(d.hooks.map(([k, v]) => `${k} ×${v}`).join(", "));
  }

  out.push(`\n## Timeline (tool calls)`);
  const MAX = 250;
  const shown = d.calls.length > MAX ? [...d.calls.slice(0, MAX / 2), null, ...d.calls.slice(-MAX / 2)] : d.calls;
  for (const c of shown) out.push(c ? `#${c.n} ${c.name}: ${c.what}` : `… ${d.calls.length - MAX} calls omitted …`);
  return out.join("\n");
}

const all = sessions();

if (list) {
  for (const s of all) {
    const d = digest(s.path);
    console.log(`${d.id}  ${new Date(s.mtime).toISOString().slice(0, 16)}  ${d.prompts.length}p ${d.calls.length}c  ${clean(d.title || d.prompts[0]?.text || "", 80)}`);
  }
  process.exit(0);
}

const pick = (ref) => {
  if (existsSync(ref)) return ref;
  const hit = all.filter((s) => basename(s.path).startsWith(ref));
  if (hit.length !== 1) {
    console.error(hit.length ? `'${ref}' matches ${hit.length} sessions` : `no session '${ref}' in ${dir}`);
    process.exit(1);
  }
  return hit[0].path;
};
const paths = last ? all.slice(0, last).map((s) => s.path) : args.length ? args.map(pick) : [all[0].path];
const ds = paths.map(digest);
console.log(ds.map(print).join("\n\n---\n\n"));

if (ds.length > 1) {
  // A habit seen in one session is an anecdote; in several, it is a candidate.
  const across = (key) => {
    const m = new Map();
    for (const d of ds) for (const [k] of d[key]) m.set(k, (m.get(k) || 0) + 1);
    return [...m].filter(([, v]) => v > 1).sort((a, b) => b[1] - a[1]);
  };
  console.log(`\n\n# Across ${ds.length} sessions`);
  for (const [key, title] of [["bash", "Shell commands"], ["reads", "Files read"], ["edits", "Files edited"]]) {
    const r = across(key);
    if (!r.length) continue;
    console.log(`\n## ${title} in more than one session`);
    for (const [k, v] of r.slice(0, 15)) console.log(`- ${v} sessions  ${clean(k, 140)}`);
  }
}
