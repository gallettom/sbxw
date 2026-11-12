#!/usr/bin/env node
// Claude Code PreToolUse hook (matcher: "Write" only — installed by sbxw's
// `install_artifact_hook`, see src/sbx.rs).
//
// Pushes newly created non-code deliverables into .sbxw-artifacts/, the
// folder sbxw's web UI lists and serves for download. Only blocks *creating*
// a brand-new file — editing/overwriting something that already exists is
// left alone, since by definition it's already wherever it's meant to be.

const fs = require('fs');
const path = require('path');

const EXTENSIONS = new Set([
  'md', 'markdown', 'pdf', 'png', 'jpg', 'jpeg', 'gif', 'svg', 'webp',
  'docx', 'pptx', 'xlsx', 'csv', 'html', 'txt',
]);

// Canonical repo files that legitimately belong at arbitrary locations
// (usually the repo root) regardless of the .sbxw-artifacts convention.
const EXEMPT_BASENAMES = new Set([
  'readme.md', 'readme', 'license', 'license.md', 'changelog.md',
  'contributing.md', 'claude.md', 'agents.md', 'code_of_conduct.md',
]);

// An .html file is a deliverable (a report, an export) or a web page of the
// project being built. Built is the likelier reading when it is the site's
// entry point, when it sits in a folder web projects keep pages in, or when
// web code sits beside it; only an .html with none of those is redirected.
const WEB_DIRS = new Set([
  'public', 'static', 'src', 'app', 'pages', 'views', 'templates', 'www', 'site',
  'web', 'frontend', 'client', 'html', 'assets', 'dist', 'build', 'components',
]);
const WEB_CODE = /\.(css|scss|sass|less|m?js|cjs|jsx|tsx?|vue|svelte|astro|php|py|rb|go)$|^package\.json$/i;

function isWebPage(abs, cwd) {
  if (path.basename(abs).toLowerCase() === 'index.html') return true;
  const dirs = path.relative(cwd, path.dirname(abs)).split(path.sep).map((d) => d.toLowerCase());
  if (dirs.some((d) => WEB_DIRS.has(d))) return true;
  try {
    return fs.readdirSync(path.dirname(abs)).some((f) => WEB_CODE.test(f));
  } catch (e) {
    return false;
  }
}

let raw = '';
process.stdin.on('data', (chunk) => { raw += chunk; });
process.stdin.on('end', () => {
  try {
    const input = JSON.parse(raw);
    const filePath = input.tool_input && input.tool_input.file_path;
    const cwd = input.cwd || process.cwd();
    if (!filePath) return process.exit(0);

    const abs = path.isAbsolute(filePath) ? filePath : path.join(cwd, filePath);
    const ext = path.extname(abs).slice(1).toLowerCase();
    const base = path.basename(abs).toLowerCase();
    const artifactsDir = path.join(cwd, '.sbxw-artifacts');
    const insideArtifactsDir = abs === artifactsDir || abs.startsWith(artifactsDir + path.sep);
    // The convention is a *project* rule, so it only governs the project. A
    // path outside the workspace — the agent's own ~/.claude state, a scratch
    // file in /tmp — has no .sbxw-artifacts/ to be redirected into, and the
    // message would have offered one anyway, quoting a `../../../..` relative
    // path as the offending name.
    const insideWorkspace = abs.startsWith(cwd + path.sep);
    // .claude/ is Claude Code's own config — skills, commands, agents — read
    // only from there. A SKILL.md moved into .sbxw-artifacts/ is never loaded.
    const insideClaudeDir = path.relative(cwd, abs).split(path.sep).includes('.claude');

    let alreadyExists = false;
    try { alreadyExists = fs.existsSync(abs); } catch (e) { /* fail open */ }

    if (
      insideWorkspace &&
      !insideArtifactsDir &&
      !insideClaudeDir &&
      !alreadyExists &&
      EXTENSIONS.has(ext) &&
      !EXEMPT_BASENAMES.has(base) &&
      !(ext === 'html' && isWebPage(abs, cwd))
    ) {
      const rel = path.relative(cwd, abs) || abs;
      // PreToolUse contract (per `claude /hooks`): exit code 2 blocks the
      // tool call and feeds stderr back to the model as the reason; stdout
      // is ignored for this event.
      process.stderr.write(
        `sbxw convention: non-code deliverables go in .sbxw-artifacts/, not "${rel}". ` +
        `Retry as .sbxw-artifacts/${path.basename(abs)} (or a subfolder under it).`
      );
      return process.exit(2);
    }
  } catch (e) {
    // Malformed input or unexpected shape: fail open, never block on our own error.
  }
  process.exit(0);
});
