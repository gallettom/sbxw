#!/usr/bin/env node
// check-codemap.mjs — offline validator for a code map written in the lat.md
// format (https://github.com/vercel-labs/lat.md).
//
// Checks what keeps a map worth trusting: every directory has an index, every
// section opens with an overview, and every [[wiki link]] and `@lat:` back
// reference still points at something that exists. Symbol lookup inside source
// files is regex-based — a heuristic that can miss an exotic declaration, but
// never invents one.
//
//   node check-codemap.mjs [--dir .sbxw-artifacts/codemap] [--root .] [--quiet]
//
// Exit codes: 0 clean, 1 errors found, 2 nothing to check.
//
// Written without template literals on purpose: sbx kit `content` fields reject
// dollar-brace, which is what a JS template literal is made of.

import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { join, relative, resolve, extname, sep } from "node:path";
import { execFileSync, execSync } from "node:child_process";

const argv = process.argv.slice(2);
const flag = (name, dflt) => {
  const i = argv.indexOf(name);
  return i === -1 || i + 1 >= argv.length ? dflt : argv[i + 1];
};
const ROOT = resolve(flag("--root", "."));
const MAP = flag("--dir", ".sbxw-artifacts/codemap").replace(/\/+$/, "");
const MAP_ABS = resolve(ROOT, MAP);
const MAP_NAME = MAP.split("/").pop().replace(/\.md$/, "");
const QUIET = argv.includes("--quiet");

const SOURCE_EXTS = [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".py", ".rs", ".go", ".c", ".h",
  ".java", ".kt", ".rb", ".php", ".swift", ".cs", ".scala", ".sh"];
const SYMBOL_EXTS = [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".py", ".rs", ".go", ".c", ".h"];
const SKIP = new Set([".git", "node_modules", "target", "dist", "build", ".next", ".venv",
  "vendor", "__pycache__", ".cache"]);
const LEAD_MAX = 250;
// The map's Archify diagram lives here (FORMAT.md, The diagram): JSON and
// HTML rather than markdown, so it is checked by its own rule below, not read
// as a section file.
const DIAGRAM = "diagram";
const ARCHIFY = "/home/agent/.sbxw/agent-skills/archify";

const errors = [];
const warnings = [];
const fail = (where, msg, hint) => errors.push({ where, msg, hint });
const warn = (where, msg, hint) => warnings.push({ where, msg, hint });

if (!existsSync(MAP_ABS)) {
  console.error("No map at " + MAP + "/ — run /codemap to write one, or pass --dir <path>.");
  process.exit(2);
}

/* ─── markdown parsing ──────────────────────────────────────────────────── */

// Blank out fenced code blocks, so a heading or a link inside one is not read
// as real. Inline code is NOT blanked: a heading may legitimately contain a
// `code span`, and blanking it would rename the section. Whether an inline span
// hides a link is decided per match instead — see LINK_OR_CODE.
const maskFences = (text) => {
  let fence = null;
  return text.split("\n").map((line) => {
    const m = line.match(/^\s{0,3}(```+|~~~+)/);
    if (fence) {
      if (m && line.trimStart().startsWith(fence)) fence = null;
      return "";
    }
    if (m) { fence = m[1]; return ""; }
    return line;
  });
};

// Links and inline code spans, matched in one pass. Whichever starts first
// wins, which is exactly the rule that separates `[[refs]]` (a quoted example,
// swallowed by the code span) from [[a#`b` c]] (a link whose heading contains
// code). Anything that isn't a link match is left alone.
const LINK_OR_CODE = /\[\[[^\]]+\]\]|`+[^`]*`+/g;
const linksIn = (line) => {
  const found = [];
  for (const m of line.matchAll(LINK_OR_CODE)) {
    if (m[0].startsWith("[[")) found.push(m[0].slice(2, -2));
  }
  return found;
};

const parseFrontmatter = (text) => {
  if (!text.startsWith("---\n")) return { body: text, skipped: 0, requireCodeMention: false };
  const close = text.indexOf("\n---", 3);
  if (close === -1) return { body: text, skipped: 0, requireCodeMention: false };
  const head = text.slice(4, close);
  const after = text.indexOf("\n", close + 1) + 1;
  return {
    body: text.slice(after),
    skipped: text.slice(0, after).split("\n").length - 1,
    requireCodeMention: /require-code-mention:\s*true/.test(head),
  };
};

// Section node: { title, chain, level, line, body[], children[] }
//
// The masked copy decides *whether* a line is a heading (so a `#` inside a
// fenced block is not one); the title itself always comes from the raw line.
// Reading the title off the mask blanks any `code span` in it, which silently
// renames the section — and with it every link that addresses it.
const parseSections = (text) => {
  const { body, skipped, requireCodeMention } = parseFrontmatter(text);
  const raw = body.split("\n");
  const root = { title: null, chain: [], level: 0, line: 1, body: [], children: [] };
  const stack = [root];
  maskFences(body).forEach((masked, i) => {
    const m = /^\s{0,3}#{1,6}(\s|$)/.test(masked)
      ? raw[i].match(/^\s{0,3}(#{1,6})\s+(.+?)\s*#*\s*$/)
      : null;
    if (!m) { stack[stack.length - 1].body.push(raw[i]); return; }
    const level = m[1].length;
    const title = m[2].trim();
    while (stack.length > 1 && stack[stack.length - 1].level >= level) stack.pop();
    const parent = stack[stack.length - 1];
    const node = { title, chain: parent.chain.concat([title]), level,
      line: i + 1 + skipped, body: [], children: [] };
    parent.children.push(node);
    stack.push(node);
  });
  return { root, requireCodeMention };
};

const walk = (dir, out = []) => {
  for (const name of readdirSync(dir)) {
    if (SKIP.has(name)) continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else out.push(p);
  }
  return out;
};

/* ─── load the map ──────────────────────────────────────────────────────── */

const files = new Map();   // "sub/dir/name" without .md  ->  record
for (const abs of walk(MAP_ABS)) {
  const rel = relative(MAP_ABS, abs).split(sep).join("/");
  if (rel.indexOf(DIAGRAM + "/") === 0) continue;
  if (!rel.endsWith(".md")) {
    fail(MAP + "/" + rel, "only .md files belong in the map",
      "keep other deliverables elsewhere under .sbxw-artifacts/");
    continue;
  }
  const text = readFileSync(abs, "utf8");
  const parsed = parseSections(text);
  files.set(rel.slice(0, -3), {
    key: rel.slice(0, -3), rel, text,
    root: parsed.root, requireCodeMention: parsed.requireCodeMention,
  });
}
if (files.size === 0) {
  console.error(MAP + "/ has no markdown files.");
  process.exit(2);
}

const byStem = new Map();
for (const key of files.keys()) {
  const stem = key.split("/").pop();
  if (!byStem.has(stem)) byStem.set(stem, []);
  byStem.get(stem).push(key);
}

const eachSection = (file, fn) => (function visit(n) { fn(n); n.children.forEach(visit); })(file.root);
const idOf = (file, node) => [MAP + "/" + file.key].concat(node.chain).join("#");
const at = (file, node) => MAP + "/" + file.rel + ":" + node.line;

/* ─── rule: every section opens with an overview ────────────────────────── */

const withoutLinks = (s) => s.replace(/\[\[[^\]]*\]\]/g, "");

for (const file of files.values()) {
  eachSection(file, (node) => {
    // A file that opens straight into its H1 is fine — that heading carries the
    // overview. Only a file with no content and no heading is empty.
    if (!node.chain.length && node.children.length) return;
    const label = node.chain.length ? idOf(file, node) : MAP + "/" + file.key + " (file intro)";
    const where = at(file, node);
    const para = [];
    for (const line of node.body) {
      if (line.trim() === "") { if (para.length) break; continue; }
      para.push(line.trim());
    }
    if (para.length === 0) {
      fail(where, '"' + label + '" has no leading paragraph',
        "one sentence right after the heading, before any child heading or list");
      return;
    }
    if (/^([-*+]\s|\d+\.\s|>|\||```|~~~|<)/.test(para[0])) {
      warn(where, '"' + label + '" opens with a list, quote or code block instead of a sentence',
        "a plain sentence reads better in search results and previews");
      return;
    }
    // The length cap keeps a section's essence quotable. An index file's intro
    // has no heading of its own and is exempt.
    if (!node.chain.length) return;
    const len = withoutLinks(para.join(" ")).trim().length;
    if (len > LEAD_MAX)
      fail(where, 'leading paragraph of "' + label + '" is ' + len + " chars (max " + LEAD_MAX + ")",
        "keep the overview to a sentence or two; detail goes in the next paragraph");
  });
}

/* ─── rule: one index per directory, listing everything in it ───────────── */

const dirs = new Set([""]);
for (const key of files.keys()) {
  const parts = key.split("/");
  for (let i = 1; i < parts.length; i++) dirs.add(parts.slice(0, i).join("/"));
}

for (const dir of dirs) {
  const dirName = dir === "" ? MAP_NAME : dir.split("/").pop();
  const indexKey = dir === "" ? MAP_NAME : dir + "/" + dirName;
  const expected = new Set();
  for (const key of files.keys()) {
    if (dir !== "" && key.indexOf(dir + "/") !== 0) continue;
    const rest = dir === "" ? key : key.slice(dir.length + 1);
    const head = rest.split("/")[0];
    if (rest.indexOf("/") !== -1 || key !== indexKey) expected.add(head);
  }
  const index = files.get(indexKey);
  if (!index) {
    const snippet = Array.from(expected).sort()
      .map((e) => "- [[" + e + "]] — what it covers, in one sentence").join("\n");
    fail(MAP + "/" + (dir ? dir + "/" : "") + dirName + ".md",
      "no index file for " + (dir === "" ? MAP + "/" : dir + "/"),
      "create it — an index is named after its own directory — starting from:\n" + snippet);
    continue;
  }
  const listed = new Set();
  for (const line of maskFences(index.text))
    for (const raw of linksIn(line))
      listed.add(raw.split(/[|#]/)[0].trim().replace(/\.md$/, "").split("/").pop());
  for (const e of expected)
    if (!listed.has(e))
      fail(MAP + "/" + index.rel, 'the index does not list "' + e + '"',
        "add:  - [[" + e + "]] — …");
  for (const l of listed)
    if (!expected.has(l) && !byStem.has(l) && l !== dirName)
      fail(MAP + "/" + index.rel, 'the index lists "' + l + '", which no longer exists',
        "drop the stale entry");
}

/* ─── source symbols (heuristic) ────────────────────────────────────────── */

const patternsFor = (ext, name) => {
  // Escape every non-word character — a dollar-brace class would be rejected
  // by the kit loader that ships this file.
  const n = name.replace(/[^A-Za-z0-9_]/g, (ch) => "\\" + ch);
  const js = ["\\b(?:function|class|const|let|var|type|interface|enum)\\s+" + n + "\\b",
    "^\\s*(?:export\\s+)?(?:default\\s+)?(?:async\\s+)?(?:get|set|static|\\*)?\\s*" + n + "\\s*[(<]",
    "\\b" + n + "\\s*[:=]\\s*(?:async\\s*)?[(<]"];
  if ([".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"].includes(ext)) return js;
  if (ext === ".py") return ["^\\s*(?:async\\s+)?def\\s+" + n + "\\b", "^\\s*class\\s+" + n + "\\b",
    "^" + n + "\\s*[:=]"];
  if (ext === ".rs") return ["\\b(?:fn|struct|enum|trait|const|static|type|mod)\\s+" + n + "\\b",
    "macro_rules!\\s*" + n + "\\b", "\\bimpl\\b[^\\n{]*\\b" + n + "\\b"];
  if (ext === ".go") return ["\\bfunc\\s+(?:\\([^)]*\\)\\s*)?" + n + "\\b",
    "\\b(?:type|const|var)\\s+" + n + "\\b"];
  if (ext === ".c" || ext === ".h") return ["\\b" + n + "\\s*\\(", "\\b(?:struct|enum|union)\\s+" + n + "\\b",
    "#\\s*define\\s+" + n + "\\b", "\\b" + n + "\\s*(?:\\[[^\\]]*\\])?\\s*[;=]"];
  return js;
};

// Every tracked file in the repo, keyed by basename: the raw material for
// "did you mean src/lib/models/models.ts?". Built once, and only when
// something has already failed to resolve.
let repoIndex = null;
const repoFilesByName = () => {
  if (repoIndex) return repoIndex;
  repoIndex = new Map();
  let files;
  try {
    files = execSync("git ls-files -z", { cwd: ROOT, encoding: "utf8",
      maxBuffer: 256 * 1024 * 1024, stdio: ["ignore", "pipe", "ignore"] })
      .split("\0").filter(Boolean);
  } catch (e) {
    files = walk(ROOT).map((p) => relative(ROOT, p).split(sep).join("/"));
  }
  for (const f of files) {
    const base = f.split("/").pop();
    if (!repoIndex.has(base)) repoIndex.set(base, []);
    repoIndex.get(base).push(f);
  }
  return repoIndex;
};

const sources = new Map();
const readSource = (rel) => {
  if (sources.has(rel)) return sources.get(rel);
  let text = null;
  try {
    const abs = resolve(ROOT, rel);
    if (statSync(abs).isFile()) text = readFileSync(abs, "utf8");
  } catch (e) { /* missing */ }
  sources.set(rel, text);
  return text;
};

const checkCodeLink = (target, where) => {
  const parts = target.split("#");
  const path = parts[0];
  const symbols = parts.slice(1);
  const text = readSource(path);
  if (text === null) {
    const hits = repoFilesByName().get(path.split("/").pop()) || [];
    const rest = symbols.length ? "#" + symbols.join("#") : "";
    const hint = hits.length === 1
      ? "did you mean [[" + hits[0] + rest + "]]? paths are relative to the repository root"
      : hits.length > 1
        ? "paths are relative to the repository root — candidates: " + hits.slice(0, 5).join(", ")
        : "paths are relative to the repository root, not to the map file";
    fail(where, "[[" + target + "]] — no such file: " + path, hint);
    return;
  }
  const ext = extname(path);
  if (symbols.length && !SYMBOL_EXTS.includes(ext)) {
    warn(where, "[[" + target + "]] — symbols in " + ext + " files are not checked",
      "the file exists; the symbol is taken on trust");
    return;
  }
  for (const sym of symbols)
    if (!patternsFor(ext, sym).some((p) => new RegExp(p, "m").test(text)))
      fail(where, "[[" + target + "]] — no definition of \"" + sym + "\" in " + path,
        "renamed or deleted? point the link at whatever replaced it");
};

/* ─── rule: wiki links resolve ──────────────────────────────────────────── */

const resolveSection = (target) => {
  const parts = target.split("#");
  const chain = parts.slice(1);
  let p = parts[0].trim().replace(/^\.\//, "").replace(/\.md$/, "");
  if (p.indexOf(MAP + "/") === 0) p = p.slice(MAP.length + 1);
  let key = files.has(p) ? p : null;
  if (!key && p.indexOf("/") === -1) {
    const cands = byStem.get(p) || [];
    if (cands.length === 1) key = cands[0];
    else if (cands.length > 1)
      return { error: '"' + p + '" is ambiguous — ' + cands.join(", "), hint: "use the full path form" };
  }
  if (!key) return { error: 'no map file named "' + parts[0] + '"' };
  const file = files.get(key);

  // Walk the heading chain from `start`; matching is case-insensitive.
  const descend = (start) => {
    let node = start;
    for (const part of chain) {
      const want = part.trim().toLowerCase();
      const next = node.children.find((c) => c.title.toLowerCase() === want);
      if (!next) return { failedAt: node, want: part.trim() };
      node = next;
    }
    return { node };
  };

  const first = descend(file.root);
  if (first.node) return { file, node: first.node };
  // The H1 may be left implicit: [[cli#mcp]] means cli.md → "# CLI" → "## mcp".
  for (const h1 of file.root.children) {
    const viaRoot = descend(h1);
    if (viaRoot.node) return { file, node: viaRoot.node };
  }
  // Nothing matched. Most misses are a link that dropped the backticks or the
  // punctuation of a heading it otherwise names correctly, so before listing
  // what exists, look for the one heading that differs only in those.
  const flat = [];
  (function collect(node) {
    for (const child of node.children) { flat.push(child); collect(child); }
  })(file.root);
  const norm = (t) => t.toLowerCase().replace(/[^a-z0-9]+/g, " ").trim();
  const wanted = norm(chain.join(" "));
  const tail = norm(chain[chain.length - 1]);
  const close = flat.filter((sec) => norm(sec.chain.join(" ")) === wanted || norm(sec.title) === tail);
  const under = MAP + "/" + file.key + first.failedAt.chain.map((c) => "#" + c).join("");
  const spelling = parts[0].trim();
  // Quote the short form the map itself uses: a lone H1 stays implicit.
  const suggestChain = file.root.children.length === 1 ? close.length && close[0].chain.slice(1) : close.length && close[0].chain;
  const hint = close.length === 1
    ? 'did you mean [[' + spelling + "#" + suggestChain.join("#")
      + ']]? a heading is matched verbatim — backticks and punctuation included'
    : flat.length
      ? "headings in that file: " + flat.slice(0, 8).map((sec) => sec.chain.join("#")).join(", ")
      : "that file has no headings";
  return { error: 'no section "' + first.want + '" under ' + under, hint };
};

let links = 0;
for (const file of files.values()) {
  maskFences(file.text).forEach((line, i) => {
    for (const raw of linksIn(line)) {
      const target = raw.split("|")[0].trim();
      if (!target) continue;
      links++;
      const where = MAP + "/" + file.rel + ":" + (i + 1);
      if (SOURCE_EXTS.includes(extname(target.split("#")[0]))) { checkCodeLink(target, where); continue; }
      const r = resolveSection(target);
      if (r.error) fail(where, "broken link [[" + target + "]] — " + r.error, r.hint);
    }
  });
}

/* ─── rule: @lat back references ────────────────────────────────────────── */

let tracked = [];
try {
  // stderr silenced: outside a repository this is an expected miss, not news.
  tracked = execSync("git ls-files -z", { cwd: ROOT, encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024, stdio: ["ignore", "pipe", "ignore"] })
    .split("\0").filter(Boolean);
} catch (e) {
  tracked = walk(ROOT).map((p) => relative(ROOT, p).split(sep).join("/"));
}
const codeFiles = tracked.filter((p) => SOURCE_EXTS.includes(extname(p))
  && p.indexOf(MAP + "/") !== 0 && !p.split("/").some((s) => SKIP.has(s)));

const mentions = new Map();
let refs = 0;
for (const p of codeFiles) {
  const text = readSource(p);
  if (text === null || text.indexOf("@lat:") === -1) continue;
  text.split("\n").forEach((line, i) => {
    const m = line.match(/(?:\/\/|#|--|;|\*)\s*@lat:\s*\[\[([^\]]+)\]\]/);
    if (!m) return;
    refs++;
    const target = m[1].split("|")[0].trim();
    const where = p + ":" + (i + 1);
    const r = resolveSection(target);
    if (r.error) { fail(where, "@lat: [[" + target + "]] — " + r.error, r.hint); return; }
    const id = idOf(r.file, r.node);
    if (!mentions.has(id)) mentions.set(id, []);
    mentions.get(id).push(where);
  });
}

for (const file of files.values()) {
  if (!file.requireCodeMention) continue;
  eachSection(file, (node) => {
    if (node.children.length || !node.chain.length) return;      // leaves only
    const hits = mentions.get(idOf(file, node)) || [];
    const name = node.chain.join(" › ");
    if (hits.length === 0) {
      // Quote the short form the map itself uses: a lone H1 stays implicit.
      const chain = file.root.children.length === 1 ? node.chain.slice(1) : node.chain;
      const short = [file.key.split("/").pop()].concat(chain).join("#");
      fail(at(file, node), 'spec "' + name + '" is not referenced by any code',
        // Split, so a scan of this file does not read the hint as a reference.
        "add next to the covering test:  // @" + "lat: [[" + short + "]]");
    } else if (hits.length > 1) {
      warn(at(file, node), 'spec "' + name + '" is referenced ' + hits.length + " times",
        "one reference per spec: " + hits.join(", "));
    }
  });
}

/* ─── rule: the Archify diagram, when there is one ──────────────────────── */

// The diagram makes the same kind of claim a code link does — this component
// is that code — so it rots the same way, and is held to the same rule: every
// file it cites still exists, and the render is not older than its source.
let diagramNote = "";
const DIAGRAM_ABS = join(MAP_ABS, DIAGRAM);
if (existsSync(DIAGRAM_ABS)) {
  const jsonAbs = join(DIAGRAM_ABS, "architecture.json");
  const htmlAbs = join(DIAGRAM_ABS, "architecture.html");
  const where = MAP + "/" + DIAGRAM + "/architecture.json";
  let candidate = null;
  if (!existsSync(jsonAbs)) {
    fail(MAP + "/" + DIAGRAM + "/", "no architecture.json — the diagram has no source",
      "draw it with /codemap step 6, or delete " + DIAGRAM + "/");
  } else {
    try { candidate = JSON.parse(readFileSync(jsonAbs, "utf8")); }
    catch (e) { fail(where, "not valid JSON — " + e.message, "fix it, then run Archify's finalize again"); }
  }
  if (candidate) {
    const html = MAP + "/" + DIAGRAM + "/architecture.html";
    if (!existsSync(htmlAbs))
      fail(html, "never rendered", "run Archify's finalize — FORMAT.md, The diagram");
    else if (statSync(htmlAbs).mtimeMs < statSync(jsonAbs).mtimeMs)
      fail(html, "older than architecture.json — the render is stale", "run Archify's finalize again");

    const headings = new Set();
    for (const file of files.values())
      eachSection(file, (node) => { if (node.title) headings.add(node.title.toLowerCase()); });

    const cited = new Set();
    const checkSources = (what, list) => {
      for (const ref of Array.isArray(list) ? list : []) {
        if (!ref || typeof ref.path !== "string") continue;
        cited.add(ref.path);
        const text = readSource(ref.path);
        if (text === null) {
          fail(where, what + " cites " + ref.path + ", which does not exist",
            "moved or deleted? cite whatever replaced it, then render again");
          continue;
        }
        const last = Math.max(ref.line || 0, ref.end_line || 0);
        const count = text.split("\n").length - (text.endsWith("\n") ? 1 : 0);
        if (last > count)
          fail(where, what + " cites " + ref.path + ":" + last + ", past its last line (" + count + ")",
            "the code moved — re-read it and fix the range");
      }
    };

    const components = Array.isArray(candidate.components) ? candidate.components : [];
    for (const c of components) {
      if (!c) continue;
      const label = typeof c.label === "string" ? c.label : "";
      if (label && !headings.has(label.toLowerCase()))
        warn(where, 'component "' + label + '" matches no heading in the map',
          "label it with the heading of the section that explains it");
      checkSources('component "' + (label || c.id) + '"', c.sources);
    }
    for (const r of Array.isArray(candidate.connections) ? candidate.connections : [])
      if (r) checkSources("relationship " + r.from + " → " + r.to, r.sources);

    // Drawn at an older commit is normal — every commit after the drawing
    // makes it so. What is worth a look is a commit that touched code the
    // diagram cites, so that is what is asked of git.
    const repo = candidate.meta && candidate.meta.repository;
    const git = (args) => execFileSync("git", args, { cwd: ROOT, encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024, stdio: ["ignore", "pipe", "ignore"] }).trim();
    if (repo && typeof repo.revision === "string" && cited.size) {
      let head = "";
      try { head = git(["rev-parse", "HEAD"]); } catch (e) { /* not a repository */ }
      if (head && head.toLowerCase() !== repo.revision.toLowerCase()) {
        let touched = null;
        try {
          touched = git(["diff", "--name-only", repo.revision, "HEAD", "--"].concat(Array.from(cited)))
            .split("\n").filter(Boolean);
        } catch (e) { /* the revision is not in this clone */ }
        if (touched === null)
          warn(where, "drawn at " + repo.revision.slice(0, 12) + ", which this clone does not have",
            "check the components against HEAD, then render again");
        else if (touched.length)
          warn(where, "cited code changed since the diagram was drawn (" + repo.revision.slice(0, 12)
            + "): " + touched.slice(0, 5).join(", ") + (touched.length > 5 ? ", …" : ""),
            "if a component or relationship moved, edit the candidate and render again");
      }
    }
    diagramNote = ", " + components.length + " diagram components";
  }
} else if (MAP_NAME === "codemap" && existsSync(ARCHIFY)) {
  warn(MAP + "/", "no " + DIAGRAM + "/ — the map has no picture of the whole yet",
    "/codemap step 6 draws one with Archify");
}

/* ─── coverage hint: source trees the map never mentions ────────────────── */

const mentioned = new Set();
for (const file of files.values())
  for (const m of file.text.matchAll(/\[\[([^\]|#]+)/g)) mentioned.add(m[1].trim().split("/")[0]);
const topDirs = new Map();
for (const p of codeFiles) {
  const top = p.indexOf("/") === -1 ? "." : p.split("/")[0];
  topDirs.set(top, (topDirs.get(top) || 0) + 1);
}
const unmapped = Array.from(topDirs.entries())
  .filter((e) => e[1] >= 3 && e[0] !== "." && !mentioned.has(e[0]))
  .sort((a, b) => b[1] - a[1]);

/* ─── report ────────────────────────────────────────────────────────────── */

const print = (items, label) => {
  if (!items.length) return;
  console.log("\n" + label + " (" + items.length + "):\n");
  for (const it of items) {
    console.log("  " + it.where);
    console.log("    " + it.msg);
    if (it.hint) console.log(it.hint.split("\n").map((l) => "      " + l).join("\n"));
    console.log("");
  }
};

let sections = 0;
for (const f of files.values()) eachSection(f, (n) => { if (n.chain.length) sections++; });

print(errors, "Errors");
print(warnings, "Warnings");

if (unmapped.length && !QUIET) {
  console.log("\nNever mentioned in the map (" + unmapped.length + "):\n");
  for (const e of unmapped.slice(0, 10)) console.log("  " + e[0] + "/  — " + e[1] + " source files");
  console.log("\n  Not an error: map what carries meaning, skip what doesn't.\n");
}

if (!QUIET || errors.length) {
  console.log(MAP + "/: " + files.size + " files, " + sections + " sections, "
    + links + " links, " + refs + " @lat refs" + diagramNote);
  console.log(errors.length
    ? "FAIL — " + errors.length + " error(s), " + warnings.length + " warning(s)"
    : "OK — " + warnings.length + " warning(s)");
}
process.exit(errors.length ? 1 : 0);
