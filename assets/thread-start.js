#!/usr/bin/env node
// Claude Code SessionStart hook, installed by sbxw beside the relay (see
// crates/core/src/tooling.rs). It acts only in a session sbxw started as a
// thread (SBXW_THREAD set): sessions restart (a daemon restart, a
// compaction), and a thread should know the state of its worktree without
// the lead having to tell it. Everywhere else it exits at once, silent.
//
// It prints, as additional context: the thread's name, branch and tip,
// `git status --short` (30 lines at most), scratch worktrees registered under
// this worktree, leftover target* dirs, whether main has commits the branch
// lacks, and one line on committing before long builds.
//
// Read-only git commands only, a hard limit of about 2 seconds in all, and
// silent on any error: a hook that fails must never stop a session.

const THREAD = process.env.SBXW_THREAD || "";
if (!THREAD) process.exit(0);

const fs = require("fs");
const path = require("path");
const { execFileSync } = require("child_process");

const DEADLINE = Date.now() + 1800;
const MAX_STATUS = 30;

function git(args) {
  const left = DEADLINE - Date.now();
  if (left <= 50) throw new Error("out of time");
  return execFileSync("git", ["--no-optional-locks", ...args], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
    timeout: left,
    maxBuffer: 4 << 20,
  });
}

function tryGit(args) {
  try {
    return git(args);
  } catch (_) {
    return null;
  }
}

function main() {
  const root = git(["rev-parse", "--show-toplevel"]).trim();
  const branch = (tryGit(["symbolic-ref", "--short", "-q", "HEAD"]) || "").trim() || "(detached)";
  const tip = (tryGit(["log", "-1", "--format=%h %s"]) || "").trim();
  const lines = [`[sbxw] Thread "${THREAD}": branch ${branch}, tip ${tip || "(none)"}.`];

  const status = (tryGit(["status", "--short"]) || "").split("\n").filter(Boolean);
  if (status.length === 0) {
    lines.push("Worktree clean.");
  } else {
    lines.push(`Uncommitted (${status.length}):`);
    for (const l of status.slice(0, MAX_STATUS)) lines.push("  " + l);
    if (status.length > MAX_STATUS) lines.push(`  ... and ${status.length - MAX_STATUS} more`);
  }

  // The worktrees git knows: the first is the main checkout; any under this
  // one is a scratch worktree left behind.
  const porcelain = tryGit(["worktree", "list", "--porcelain"]) || "";
  const trees = [];
  for (const block of porcelain.split("\n\n")) {
    const wt = /^worktree (.+)$/m.exec(block);
    if (!wt) continue;
    const br = /^branch refs\/heads\/(.+)$/m.exec(block);
    trees.push({ path: wt[1], branch: br ? br[1] : null });
  }
  const scratch = trees.filter((t) => t.path.startsWith(root + path.sep)).map((t) => path.relative(root, t.path));
  if (scratch.length) lines.push(`Scratch worktrees under this one: ${scratch.join(", ")}.`);

  const targets = [];
  for (const dir of [root, path.join(root, "desktop")]) {
    let names = [];
    try {
      names = fs.readdirSync(dir, { withFileTypes: true });
    } catch (_) {}
    for (const d of names) {
      if (d.isDirectory() && d.name.startsWith("target")) targets.push(path.relative(root, path.join(dir, d.name)));
    }
  }
  if (targets.length) lines.push(`Leftover target dirs: ${targets.join(", ")} (delete them when you are done).`);

  // Main: the branch of the main checkout, when it is another branch.
  const mainBranch = trees.length && trees[0].path !== root ? trees[0].branch : null;
  if (mainBranch && mainBranch !== branch) {
    const behind = parseInt((tryGit(["rev-list", "--count", `HEAD..refs/heads/${mainBranch}`]) || "").trim(), 10);
    if (behind > 0) {
      lines.push(`${mainBranch} has ${behind} commit(s) this branch lacks: check \`git merge-tree --write-tree ${mainBranch} HEAD\` before you report done.`);
    } else if (behind === 0) {
      lines.push(`${mainBranch} has no commit this branch lacks.`);
    }
  }
  lines.push("Commit before long builds, so a restart costs little.");
  return lines.join("\n");
}

try {
  const text = main();
  process.stdout.write(JSON.stringify({ hookSpecificOutput: { hookEventName: "SessionStart", additionalContext: text } }));
} catch (_) {
  // Silent: no context is better than a broken start.
}
process.exit(0);
