//! The shell scripts sbxw runs inside a sandbox for threads and code maps.
//!
//! Every variable part — workspace, thread name, prompt, model, sandbox name,
//! port — is a **positional argument** (`$1`…), never spliced into script
//! text: names arrive from HTTP paths and WebSocket queries, a lens brief is a
//! sentence a human typed, and nothing downstream would notice if one of them
//! became a command.

use sbxw_core::{ModelId, SandboxName, ThreadName};

fn owned(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// `sbx exec` refuses an empty argv element ("cmd element N is empty"), and a
/// thread's workspace, prompt or model is often empty. So the thread scripts'
/// positional values go in behind a `=` (`pos`), and this, first in each
/// script, takes it off again: `$1`… are the values as given, empty included.
macro_rules! unwrap_args {
    () => {
        r#"
        for a do set -- "$@" "${a#=}"; shift; done"#
    };
}

/// One positional value of a thread script, wrapped for `unwrap_args!`.
fn pos(value: &str) -> String {
    format!("={value}")
}

/// Shell function `sbxw_no_repo`: why the current directory cannot hold a
/// thread, and the repos under it (a workspace is often a folder of repos; the
/// fix is to pick one, `repo=` on the thread's socket). One line, no colour:
/// each script frames it its own way.
macro_rules! no_repo_fn {
    () => {
        r#"
        sbxw_no_repo() {
            found=$(find . -maxdepth 3 -name .git -not -path '*/.sbxw-worktrees/*' -not -path '*/node_modules/*' 2>/dev/null \
                | sed -e 's|^\./||' -e 's|/\.git$||' | sort | tr '\n' ' ')
            if [ -n "$found" ]; then
                printf '%s is not a git repository - pick a repo for this thread. Repos under it: %s' "$PWD" "${found% }"
            else
                printf '%s is not a git repository, and none was found under it: a thread needs one for its worktree.' "$PWD"
            fi
        }"#
    };
}

/// Shell function `sbxw_worktree <root> <key> <shared>`: make sure the
/// worktree `<root>/.sbxw-worktrees/<key>` exists, on branch `sbxw/<key>`
/// (made from `HEAD` when it does not exist yet), and set `wt` to it. On a
/// failure it returns 1 with the reason in `sbxw_err`, one line.
///
/// * `.sbxw-worktrees/` goes in the repo's `info/exclude`, not `.gitignore`:
///   hiding sbxw's bookkeeping must not be a change someone has to commit.
/// * `<shared>` = 1 for a shared plan's worktree, which its threads all ask
///   for at once: one makes it, under a lock in the git dir.
/// * `.sbxw-worktrees` is made before `git worktree add`, so a root the agent
///   cannot write is said in plain words rather than as git's "could not
///   create leading directories". Nothing tries harder: not `sudo` (the
///   sandbox gives it for installing packages, not for writing the person's
///   repository), not a folder elsewhere. A workspace root the agent cannot
///   write is most often one the sandbox did not mount at all (only some of
///   its folders are), and a worktree made there or anywhere else outside the
///   mounted repository would live in the container only: the host's git
///   would not see it (the worktree graph, `git worktree prune` there) and
///   the sandbox's removal would take the thread's uncommitted work with it.
///   When `.git` sits on another filesystem than the root, the reason says
///   so: that is the mark of such a partial mount, read, not guessed.
macro_rules! worktree_fn {
    () => {
        r#"
        sbxw_worktree() {
            local root="$1" key="$2" shared="$3" wtd="$1/.sbxw-worktrees" ex out who owner note=""
            wt="$wtd/$key"; sbxw_err=""
            [ -d "$wt" ] && return 0
            if [ "$shared" = 1 ] && command -v flock >/dev/null; then
                exec 9>"$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)/sbxw-$key.lock"
                flock -w 60 9
            fi
            if [ -d "$wt" ]; then exec 9>&-; return 0; fi
            if ! mkdir -p "$wtd" 2>/dev/null || [ ! -w "$wtd" ]; then
                who=$(id -un 2>/dev/null || id -u); owner=$(stat -c %U "$root" 2>/dev/null || echo unknown)
                if [ -e "$root/.git" ] && [ "$(stat -c %d "$root" 2>/dev/null)" != "$(stat -c %d "$root/.git" 2>/dev/null)" ]; then
                    note=" $root/.git is mounted separately from $root itself, so this sandbox most likely mounts only some of the workspace's folders, not the workspace."
                fi
                sbxw_err="cannot create $wtd: $root is not writable by $who in this sandbox (owner $owner).$note A thread's worktree has to be made there: give the sandbox the whole repository, writable, then open the thread again."
                exec 9>&-; return 1
            fi
            ex=$(git -C "$root" rev-parse --git-path info/exclude)
            case "$ex" in /*) ;; *) ex="$root/$ex" ;; esac
            mkdir -p "$(dirname "$ex")"
            grep -qxF '/.sbxw-worktrees/' "$ex" 2>/dev/null || printf '/.sbxw-worktrees/\n' >> "$ex"
            if git -C "$root" show-ref --verify --quiet "refs/heads/sbxw/$key"; then
                out=$(git -C "$root" worktree add "$wt" "sbxw/$key" 2>&1)
            else
                out=$(git -C "$root" worktree add -b "sbxw/$key" "$wt" 2>&1)
            fi
            if [ $? -ne 0 ]; then
                sbxw_err="git worktree add $wt failed: $(printf '%s' "$out" | grep -v '^Preparing' | tail -n 1)"
                exec 9>&-; return 1
            fi
            exec 9>&-
            return 0
        }"#
    };
}

/// Shell function `sbxw_merge_check <root> <git-common-dir> <branch>`: may
/// `<branch>` be merged into the main checkout `<root>` as it stands? On a
/// refusal it prints the `error=` line and returns 1. Otherwise it sets
/// `absent_n` (tracked files missing from the working tree, untouched by the
/// merge) and `edits_n` (unstaged edits, untouched by the merge). The rules
/// are `THREAD_MERGE_SCRIPT`'s; the plan dialog's count of uncommitted work
/// (`CHECKOUT_STATE_SCRIPT`) classifies the same way.
macro_rules! merge_check_fn {
    () => {
        r#"
        sbxw_names() { # the first 5 of "$@", space-separated, then "and N more"
            local out="" n=0
            for p do n=$((n + 1)); [ "$n" -le 5 ] && out="$out${out:+, }$p"; done
            [ "$n" -gt 5 ] && out="$out and $((n - 5)) more"
            printf '%s' "$out"
        }
        sbxw_merge_check() {
            local root="$1" common="$2" br="$3" f p st mb
            for f in MERGE_HEAD CHERRY_PICK_HEAD REVERT_HEAD rebase-merge rebase-apply; do
                if [ -e "$common/$f" ]; then
                    echo "error=the main checkout of $root is in the middle of a merge, rebase, cherry-pick or revert ($f): finish or abort it there first"
                    return 1
                fi
            done
            local -a unmerged=() staged=() edits=() absent=() hit_edits=() hit_absent=()
            while IFS= read -r -d '' p; do unmerged+=("$p"); done < <(git -C "$root" diff --name-only -z --diff-filter=U)
            if [ "${#unmerged[@]}" -gt 0 ]; then
                echo "error=the main checkout of $root has unresolved conflicts ($(sbxw_names "${unmerged[@]}")): resolve them there first"
                return 1
            fi
            local -A touched=()
            if mb=$(git -C "$root" merge-base HEAD "$br" 2>/dev/null); then
                while IFS= read -r -d '' p; do touched[$p]=1; done < <(git -C "$root" diff --name-only -z --no-renames "$mb" "$br" --)
            else
                while IFS= read -r -d '' p; do touched[$p]=1; done < <(git -C "$root" ls-tree -r -z --name-only "$br")
            fi
            while IFS= read -r -d '' p; do staged+=("$p"); done < <(git -C "$root" diff --cached --name-only -z --no-renames)
            while IFS= read -r -d '' st && IFS= read -r -d '' p; do
                if [ "$st" = D ]; then
                    if [ -n "${touched[$p]:-}" ]; then hit_absent+=("$p"); else absent+=("$p"); fi
                else
                    if [ -n "${touched[$p]:-}" ]; then hit_edits+=("$p"); else edits+=("$p"); fi
                fi
            done < <(git -C "$root" diff --name-status -z --no-renames)
            if [ "${#hit_absent[@]}" -gt 0 ]; then
                echo "error=${#hit_absent[@]} file(s) this merge changes are missing from the main checkout of $root in this sandbox ($(sbxw_names "${hit_absent[@]}")). The repository is most likely only partly mounted here: do this merge where the whole repository is present. Do not commit these as deletions: that would delete real files."
                return 1
            fi
            if [ "${#staged[@]}" -gt 0 ]; then
                echo "error=the main checkout of $root has ${#staged[@]} staged change(s) ($(sbxw_names "${staged[@]}")), and git will not merge into an index that differs from HEAD: commit or unstage those first"
                return 1
            fi
            if [ "${#hit_edits[@]}" -gt 0 ]; then
                echo "error=the main checkout of $root has uncommitted changes to ${#hit_edits[@]} file(s) this merge changes ($(sbxw_names "${hit_edits[@]}")): commit or stash those files first"
                return 1
            fi
            absent_n=${#absent[@]}
            edits_n=${#edits[@]}
            return 0
        }"#
    };
}

/// Argv for a *thread*: a second Claude session in `name`, in its own git
/// worktree `<repo>/.sbxw-worktrees/<thread>` on branch `sbxw/<thread>` — two
/// agents in one working tree overwrite each other's files and mix their
/// changes into one unreviewable diff. The worktree outlives the session
/// (reopening lands back in it, on its branch, uncommitted work included);
/// removing it is left to a person, since it can hold uncommitted work.
///
/// * The worktree is made by `sbxw_worktree` (see `worktree_fn!`), which the
///   daemon has already run once through `THREAD_PREPARE_SCRIPT`.
/// * `sbx exec`, not `sbx run`: `run` attaches *the* agent, so a second one
///   would be that same session. The permission flag is the one sbx gives the
///   agent it starts: a thread is exactly as trusted as the pane beside it.
///   `bash -lc` is a login shell, for `claude` on PATH (see `codemap_run_args`).
/// * A thread has at most one live session. `mode` says whether this is its
///   first start ([`StartMode::New`]) or a reopen ([`StartMode::Reopen`]):
///   its pane after the daemon or the sandbox restarted, which ends every
///   terminal sbxw held but not always the session behind it (`sbx exec`'s
///   process outlives its client). A session of the thread still running
///   with no pane is stopped first; a reopen then resumes that conversation,
///   or the thread's last one in the worktree (`claude --resume <id>`, read
///   from Claude Code's session registry by the thread's name and worktree;
///   `--continue` for a split thread whose conversation predates that), and
///   only failing both starts fresh with `prompt`: the plan's goal, said as
///   a restart. A thread never comes back blank when its plan has a goal.
/// * `SBXW_THREAD` names the thread to everything the session starts; the
///   relay MCP server reads it to know it serves a thread, not the main
///   session (the one that can lead). `prompt`, when given, is the session's
///   first message — an approved plan's briefing.
/// * `--name` makes the thread's name its address in Claude Code's own
///   messaging between sessions (the lead's `SendMessage` to `api-auth`; sbxw
///   is not on that path). Without it the session gets a name derived from its
///   folder (`api-auth-5e`) or from its first prompt, which a lead cannot predict.
/// * The worktree is marked trusted in `.claude.json` before `claude` starts,
///   as `Tooling::trust_workspace` does for the workspace: Claude Code does
///   not extend a folder's trust to the folders under it, so a new worktree
///   would open on the trust dialog instead of on its briefing.
/// * `model`, when given, is passed as `claude --model`, on a fresh start and
///   on `--continue` alike; empty leaves the sandbox's default
///   (`ANTHROPIC_DEFAULT_MODEL`) in charge.
/// * `team`, for a thread of a shared plan (Team mode), is that plan: the
///   session opens in the plan's one worktree, `.sbxw-worktrees/team-<plan>`
///   on branch `sbxw/team-<plan>`, made under a lock by whichever thread
///   starts first (they all start at once), with `SBXW_TEAM=shared` and
///   `SBXW_TEAM_PLAN=<plan>` set. Such a thread never `--continue`s: all
///   share the cwd transcripts are filed under, so "its last conversation"
///   could be another thread's. It resumes by session id only.
/// * Workspace, thread name, prompt, model, plan and mode are positional (`$1`–`$7`),
///   never script text: the thread name comes from a WebSocket query parameter.
pub fn thread_run_args(
    name: &SandboxName,
    workspace: &str,
    thread: &ThreadName,
    prompt: &str,
    model: Option<&ModelId>,
    team: Option<&Team>,
    mode: StartMode,
) -> Vec<String> {
    const SCRIPT: &str = concat!(unwrap_args!(), no_repo_fn!(), worktree_fn!(), r#"
        [ -n "$1" ] && cd "$1" 2>/dev/null
        if ! root=$(git rev-parse --show-toplevel 2>/dev/null); then
            printf '\033[31m[sbxw] %s\033[0m\r\n' "$(sbxw_no_repo)"
            exit 1
        fi
        key="${5:-$2}"
        if ! sbxw_worktree "$root" "$key" "$([ -n "$5" ] && echo 1)"; then
            printf '\033[31m[sbxw] %s\033[0m\r\n' "$sbxw_err"
            exit 1
        fi
        printf '\033[2m[sbxw] thread %s - worktree %s, branch sbxw/%s\033[0m\r\n' "$2" "$wt" "$key"
        export SBXW_THREAD="$2"
        if [ -n "$5" ]; then
            export SBXW_TEAM=shared SBXW_TEAM_PLAN="$6"
        fi
        cd "$wt" || exit 1
        # Trusted, or the session stops on "Do you trust this folder?" before
        # its briefing: a worktree is a folder Claude Code has never seen, and
        # sitting under the trusted workspace does not count. Keyed by the cwd
        # as Node sees it, which is what Claude Code looks up. Written to a
        # copy renamed over the original, its mode kept, and only the first
        # time; an unreadable file is left alone rather than replaced.
        node -e '
            const fs = require("fs"), p = process.argv[1], w = process.cwd();
            let d, st;
            try { st = fs.statSync(p); d = JSON.parse(fs.readFileSync(p, "utf8")); } catch (e) { process.exit(0); }
            const was = (d.projects || {})[w];
            if (was && was.hasTrustDialogAccepted) process.exit(0);
            d.projects = d.projects || {};
            d.projects[w] = Object.assign({}, was, { hasTrustDialogAccepted: true });
            const t = p + ".sbxw-" + process.pid;
            fs.writeFileSync(t, JSON.stringify(d, null, 2), { mode: st.mode & 0o777 });
            fs.renameSync(t, p);
        ' "${CLAUDE_CONFIG_DIR:-$HOME}/.claude.json" 2>/dev/null
        # At most one live session per thread. A session sbxw started earlier
        # can still be running in the sandbox with no pane: `sbx exec`'s
        # process outlives its client, so a daemon restart leaves it working
        # unseen. It is found in Claude Code's registry of sessions (this
        # thread's name, this worktree, a live pid whose environment says
        # SBXW_THREAD is this thread), stopped, and its conversation resumed
        # here, rather than a second session started beside it on the same
        # files. Otherwise, reopened: the thread's last conversation in this
        # worktree, by its id (a shared worktree holds every thread's), and
        # only failing that a fresh start with the goal sbxw passed.
        cfg="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
        tr="$cfg/projects/$(printf '%s' "$wt" | sed 's/[^A-Za-z0-9]/-/g')"
        set -- "$@" $(node -e '
            const fs = require("fs"), path = require("path");
            const [dir, name, cwd] = process.argv.slice(1);
            let all = [];
            try {
                all = fs.readdirSync(dir).filter((f) => /^[0-9]+\.json$/.test(f)).map((f) => {
                    try { return JSON.parse(fs.readFileSync(path.join(dir, f), "utf8")); } catch (e) { return null; }
                });
            } catch (e) {}
            const ours = all
                .filter((e) => e && e.name === name && e.cwd === cwd && Number.isInteger(e.pid))
                .filter((e) => typeof e.sessionId === "string" && /^[0-9a-f-]{8,64}$/.test(e.sessionId))
                .sort((a, b) => (b.startedAt || 0) - (a.startedAt || 0));
            // Live, and the very session sbxw started for this thread: its
            // environment says SBXW_THREAD=<thread> and its command line
            // carries --name <thread>. A pid the registry names but that is
            // now some other process fails both.
            const alive = (e) => {
                try { process.kill(e.pid, 0); } catch (x) { return false; }
                try {
                    const env = fs.readFileSync("/proc/" + e.pid + "/environ", "utf8").split("\0");
                    const argv = fs.readFileSync("/proc/" + e.pid + "/cmdline", "utf8").split("\0");
                    const named = argv.some((a, i) => a === "--name" && argv[i + 1] === name);
                    return env.includes("SBXW_THREAD=" + name) && named;
                } catch (x) { return false; }
            };
            const live = ours.find(alive);
            if (live) console.log("live " + live.pid + " " + live.sessionId);
            else if (ours[0]) console.log("last 0 " + ours[0].sessionId);
        ' "$cfg/sessions" "$2" "$wt" 2>/dev/null)
        found="${8:-}"; found_pid="${9:-}"; found_id="${10:-}"
        resume=""; resume_note=""
        if [ "$found" = live ]; then
            printf '\033[33m[sbxw] thread %s already has a session running in this sandbox (pid %s) that no pane shows (its terminal was lost, e.g. when the daemon restarted): stopping it so that one session works in this worktree.\033[0m\r\n' "$2" "$found_pid"
            kill -TERM "$found_pid" 2>/dev/null
            for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$found_pid" 2>/dev/null || break; sleep 1; done
            if kill -0 "$found_pid" 2>/dev/null; then
                kill -KILL "$found_pid" 2>/dev/null; sleep 1
            fi
            if kill -0 "$found_pid" 2>/dev/null && [ "$(cut -d' ' -f3 "/proc/$found_pid/stat" 2>/dev/null)" != Z ]; then
                printf '[31m[sbxw] the earlier session of thread %s (pid %s) did not stop: not starting a second one beside it. Stop it in the sandbox (kill -9 %s), then open the thread again.[0m
' "$2" "$found_pid" "$found_pid"
                exit 1
            fi
            if [ "$7" != new ]; then
                resume="$found_id"
                resume_note="[sbxw] Your session was stopped and resumed here by sbxw: its terminal was lost (the sbxw daemon restarted), and a second session must not work beside it. Whatever you were doing was interrupted: a command you were running was stopped, and the edits you had made are still on disk, uncommitted. Read git status and git diff in your worktree before going on, then carry on with your goal."
            fi
        elif [ "$found" = last ] && [ "$7" != new ] && [ -f "$tr/$found_id.jsonl" ]; then
            resume="$found_id"
        fi
        if [ -n "$resume" ]; then
            printf '\033[2m[sbxw] resuming its conversation (%s)\033[0m\r\n' "$resume"
            exec claude --dangerously-skip-permissions --name "$2" ${4:+--model "$4"} --resume "$resume" ${resume_note:+"$resume_note"}
        fi
        # A thread started before sbxw resumed by id: its last conversation
        # in its own worktree. Never in a shared one.
        if [ "$7" != new ] && [ -z "$5" ] && [ -z "$3" ] && ls "$tr"/*.jsonl >/dev/null 2>&1; then
            printf '\033[2m[sbxw] resuming its last conversation\033[0m\r\n'
            exec claude --dangerously-skip-permissions --name "$2" ${4:+--model "$4"} --continue
        fi
        if [ "$7" != new ] && [ -n "$3" ]; then
            printf '\033[2m[sbxw] no conversation of thread %s to resume: starting it again with its goal\033[0m\r\n' "$2"
        fi
        exec claude --dangerously-skip-permissions --name "$2" ${4:+--model "$4"} ${3:+"$3"}"#);
    let mut args = owned(&["exec", "-it", name.as_str(), "--", "bash", "-lc", SCRIPT, "sbxw-thread"]);
    let model = model.map(ModelId::as_str).unwrap_or("");
    let (worktree, plan) = team.map_or(("", ""), |t| (t.worktree.as_str(), t.plan.as_str()));
    args.extend([workspace, thread.as_str(), prompt, model, worktree, plan, mode.as_str()].map(pos));
    args
}

/// How a thread's session is being started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMode {
    /// Its first start for this plan (or a retry before it ever ran): a fresh
    /// conversation with `prompt` as its first message, whatever conversation
    /// the worktree holds from earlier.
    New,
    /// Opened again (its pane after a restart, a second open, a reopen after
    /// a failure): resume its conversation if it has one, and only failing
    /// that start fresh with `prompt`, the plan's goal said as a restart.
    Reopen,
}

impl StartMode {
    pub fn as_str(self) -> &'static str {
        match self {
            StartMode::New => "new",
            StartMode::Reopen => "reopen",
        }
    }
}

/// Make thread `$2`'s worktree (or, with `$3`, the shared plan worktree
/// `$3`) in the repo under `$1`, without starting anything: what the daemon
/// runs before a thread's session, so a failure (no repo, a root it cannot
/// write, `git worktree add` refusing) comes back as a reason it records on
/// the thread, not as a pane that printed it and died. Prints `worktree=`
/// and `ok=1`, or `error=`. The session's own script does the same again,
/// finding it made. Positional arguments, as in `thread_run_args`.
pub const THREAD_PREPARE_SCRIPT: &str = concat!(unwrap_args!(), no_repo_fn!(), worktree_fn!(), r#"
    [ -n "$1" ] && cd "$1" 2>/dev/null
    if ! root=$(git rev-parse --show-toplevel 2>/dev/null); then
        echo "error=$(sbxw_no_repo)"; exit 3
    fi
    if ! sbxw_worktree "$root" "${3:-$2}" "$([ -n "$3" ] && echo 1)"; then
        echo "error=$sbxw_err"; exit 4
    fi
    echo "worktree=$wt"
    echo ok=1"#);

/// Argv for `THREAD_PREPARE_SCRIPT`.
pub fn thread_prepare_args(name: &SandboxName, workspace: &str, thread: &ThreadName, team: Option<&Team>) -> Vec<String> {
    let mut args = owned(&["exec", name.as_str(), "--", "bash", "-lc", THREAD_PREPARE_SCRIPT, "sbxw-thread-prepare"]);
    let worktree = team.map_or("", |t| t.worktree.as_str());
    args.extend([workspace, thread.as_str(), worktree].map(pos));
    args
}

/// Argv for a session in the main checkout: another Claude beside the main
/// agent, in the workspace itself — no worktree, no branch. Two agents can
/// then overwrite each other's edits; the person chose that (a second
/// conversation about the same code), and the menu that offers it says so.
///
/// Like a thread, it runs through `sbx exec` under `SBXW_THREAD` and `--name`,
/// so its hooks are filed under its name, not read as the main agent's. It
/// never `--continue`s: it shares the main agent's cwd, so "its last
/// conversation" would be the main agent's. Workspace, name and model are
/// positional, as in `thread_run_args`.
pub fn same_worktree_run_args(name: &SandboxName, workspace: &str, thread: &ThreadName, model: Option<&ModelId>) -> Vec<String> {
    const SCRIPT: &str = concat!(unwrap_args!(), r#"
        [ -n "$1" ] && cd "$1" 2>/dev/null
        printf '\033[2m[sbxw] session %s - same checkout as the main session (%s)\033[0m\r\n' "$2" "$PWD"
        export SBXW_THREAD="$2"
        exec claude --dangerously-skip-permissions --name "$2" ${3:+--model "$3"}"#);
    let mut args = owned(&["exec", "-it", name.as_str(), "--", "bash", "-lc", SCRIPT, "sbxw-session"]);
    let model = model.map(ModelId::as_str).unwrap_or("");
    args.extend([workspace, thread.as_str(), model].map(pos));
    args
}

/// The shared plan a thread works under (Team mode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Team {
    /// Its plan's id: `SBXW_TEAM_PLAN`.
    pub plan: String,
    /// Its worktree's folder under `.sbxw-worktrees/`, and its branch after
    /// `sbxw/`: `team-<plan>`.
    pub worktree: ThreadName,
}

/// Where a thread stands, read before closing it: what closing would lose.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ThreadStatus {
    /// Its worktree, as the sandbox sees it.
    pub worktree: String,
    /// Its branch, `sbxw/<thread>`.
    pub branch: String,
    /// The branch the main checkout is on — what "merged" is measured against.
    pub base: String,
    pub has_branch: bool,
    /// Files changed in the worktree and not committed: lost on close.
    pub dirty: u32,
    /// Commits on the thread's branch that `base` does not have: lost if the
    /// branch is deleted.
    pub ahead: u32,
}

/// One script for both questions about a thread's end — "what would closing
/// lose?" (`status`) and "close it" (`close`) — so the two can never disagree
/// about which worktree, branch and base they mean.
///
/// Closing stops every process working in the worktree (found by its cwd: the
/// thread's `claude`, its shells, a dev server it started), then removes the
/// worktree, then the branch if asked. Uncommitted work is refused unless
/// `discard` is set: the browser has asked by then, but the daemon does not
/// trust a click it did not see. Positional arguments, as in `thread_run_args`.
pub const THREAD_END_SCRIPT: &str = concat!(unwrap_args!(), no_repo_fn!(), r#"
    [ -n "$1" ] && cd "$1" 2>/dev/null
    if ! common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null); then
        echo "error=$(sbxw_no_repo)"; exit 3
    fi
    root=$(dirname "$common")
    wt="$root/.sbxw-worktrees/$2"; br="sbxw/$2"
    base=$(git -C "$root" rev-parse --abbrev-ref HEAD)
    dirty=0; [ -d "$wt" ] && dirty=$(git -C "$wt" status --porcelain | wc -l | tr -d ' ')
    has_branch=0; ahead=0
    if git -C "$root" show-ref --verify --quiet "refs/heads/$br"; then
        has_branch=1; ahead=$(git -C "$root" rev-list --count "HEAD..$br")
    fi
    if [ "$3" = status ]; then
        printf 'worktree=%s\nbranch=%s\nbase=%s\nhas_branch=%s\ndirty=%s\nahead=%s\n' \
            "$wt" "$br" "$base" "$has_branch" "$dirty" "$ahead"
        exit 0
    fi
    if [ "$dirty" -gt 0 ] && [ "$4" != 1 ]; then
        echo "error=$wt has $dirty uncommitted file(s)"; exit 4
    fi
    for p in /proc/[0-9]*; do
        c=$(readlink "$p/cwd" 2>/dev/null) || continue
        pid=${p#/proc/}; [ "$pid" = $$ ] && continue
        case "$c" in "$wt"|"$wt"/*) kill "$pid" 2>/dev/null ;; esac
    done
    sleep 1
    if [ -d "$wt" ]; then
        git -C "$root" worktree remove --force "$wt" || { echo "error=could not remove $wt"; exit 5; }
    fi
    git -C "$root" worktree prune
    if [ "$5" = 1 ] && [ "$has_branch" = 1 ]; then
        git -C "$root" branch -D "$br" >/dev/null || { echo "error=could not delete $br"; exit 6; }
    fi
    echo ok=1"#);

/// Argv for a shell in thread `thread`'s worktree — what the Bash button opens
/// in a thread's pane; a shared plan's worktree for one of its threads.
/// Refuses rather than create the worktree: a shell is not how a thread is
/// started. Positional arguments, as in `thread_run_args`.
pub fn thread_bash_args(name: &SandboxName, workspace: &str, thread: &ThreadName, team: Option<&Team>) -> Vec<String> {
    const SCRIPT: &str = concat!(unwrap_args!(), no_repo_fn!(), r#"
        [ -n "$1" ] && cd "$1" 2>/dev/null
        if ! common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null); then
            printf '\033[31m[sbxw] %s\033[0m\r\n' "$(sbxw_no_repo)"; exit 1
        fi
        wt="$(dirname "$common")/.sbxw-worktrees/${3:-$2}"
        if [ ! -d "$wt" ]; then
            printf '\033[31m[sbxw] thread %s has no worktree (%s) - open the thread first.\033[0m\r\n' "$2" "$wt"
            exit 1
        fi
        cd "$wt" && exec bash -l"#);
    let mut args = owned(&["exec", "-it", name.as_str(), "--", "bash", "-lc", SCRIPT, "sbxw-thread-bash"]);
    let worktree = team.map_or("", |t| t.worktree.as_str());
    args.extend([workspace, thread.as_str(), worktree].map(pos));
    args
}

/// Argv for `THREAD_END_SCRIPT`. `action` is `status` or `close`.
pub fn thread_end_args(
    name: &SandboxName,
    workspace: &str,
    thread: &ThreadName,
    action: &str,
    discard: bool,
    delete_branch: bool,
) -> Vec<String> {
    let flag = |b: bool| if b { "1" } else { "0" };
    let mut args: Vec<String> = owned(&[
        "exec",
        name.as_str(),
        "--",
        "bash",
        "-lc",
        THREAD_END_SCRIPT,
        "sbxw-thread-end",
    ]);
    args.extend([workspace, thread.as_str(), action, flag(discard), flag(delete_branch)].map(pos));
    args
}

/// Merge thread `$2`'s branch into the branch the main checkout of the repo
/// under `$1` is on — what a lead's proposed merge runs, one thread at a time
/// (the daemon's plan merge).
///
/// Refuses only what could harm the merge or the person's work, judged
/// against the paths the merge changes (`git diff --name-only` from the
/// merge base to the branch, renames as two paths):
///
/// * a merge, rebase, cherry-pick or revert under way, or unresolved
///   conflicts: the checkout is mid-operation;
/// * staged changes, anywhere: git itself refuses a merge whose index differs
///   from `HEAD` (they would be recorded in the merge commit);
/// * uncommitted edits to a path the merge changes, named: commit or stash
///   *those*;
/// * a path the merge changes that is missing from the working tree (an
///   unstaged deletion): most often a workspace only partly mounted in the
///   sandbox. The merge must then be done where the whole repository is.
///   Committing the "deletions" would delete real files, so it is never the
///   advice.
///
/// Everything else is left as it is: unstaged edits elsewhere stay in the
/// working tree, and tracked files absent from it (` D`) that the merge does
/// not touch stay absent and stay in the commit — a merge commit records the
/// index plus the merge, and the index still holds them. Their counts come
/// back as `edits=` and `absent=` so the result can say so.
///
/// A failed merge is aborted (`git merge --abort`), so the checkout is as it
/// was, and the conflicting files are named. Prints `key=value` lines like
/// `THREAD_END_SCRIPT`: `commit=` on success, `error=` otherwise. Everything
/// variable is positional.
pub const THREAD_MERGE_SCRIPT: &str = concat!(unwrap_args!(), no_repo_fn!(), merge_check_fn!(), r#"
    [ -n "$1" ] && cd "$1" 2>/dev/null
    if ! common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null); then
        echo "error=$(sbxw_no_repo)"; exit 3
    fi
    root=$(dirname "$common"); br="sbxw/$2"
    base=$(git -C "$root" rev-parse --abbrev-ref HEAD)
    if [ "$base" = HEAD ]; then
        echo "error=the main checkout of $root is on a detached HEAD"; exit 4
    fi
    if ! git -C "$root" show-ref --verify --quiet "refs/heads/$br"; then
        echo "error=there is no branch $br in $root"; exit 5
    fi
    sbxw_merge_check "$root" "$common" "$br" || exit 6
    if out=$(git -C "$root" merge --no-ff --no-edit "$br" 2>&1); then
        echo "base=$base"
        echo "commit=$(git -C "$root" rev-parse --short HEAD)"
        echo "absent=$absent_n"
        echo "edits=$edits_n"
        echo ok=1
        exit 0
    fi
    files=$(git -C "$root" diff --name-only --diff-filter=U | head -n 5 | tr '\n' ' ')
    git -C "$root" merge --abort >/dev/null 2>&1
    if [ -n "$files" ]; then
        echo "error=conflict in ${files% }"
    else
        echo "error=$(printf '%s' "$out" | tail -n 1)"
    fi
    exit 7"#);

/// Argv for `THREAD_MERGE_SCRIPT`.
pub fn thread_merge_args(name: &SandboxName, workspace: &str, thread: &ThreadName) -> Vec<String> {
    let mut args = owned(&["exec", name.as_str(), "--", "bash", "-lc", THREAD_MERGE_SCRIPT, "sbxw-thread-merge"]);
    args.extend([workspace, thread.as_str()].map(pos));
    args
}

/// What a merge that went through has to say besides its commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeOutcome {
    /// The merge commit, short.
    pub commit: String,
    /// Tracked files missing from the main checkout's working tree that the
    /// merge did not touch: left alone, still in the commit.
    pub absent: u32,
    /// Unstaged edits in the main checkout the merge did not touch: left as
    /// they were.
    pub edits: u32,
}

impl MergeOutcome {
    /// From the `key=value` lines `THREAD_MERGE_SCRIPT` prints.
    pub fn from_fields(f: &std::collections::HashMap<String, String>) -> Self {
        let num = |k: &str| f.get(k).and_then(|v| v.parse().ok()).unwrap_or(0);
        MergeOutcome { commit: f.get("commit").cloned().unwrap_or_default(), absent: num("absent"), edits: num("edits") }
    }

    /// What the person should know about the checkout after the merge, or
    /// `None` when there is nothing to say.
    pub fn note(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.absent > 0 {
            parts.push(format!(
                "{} tracked file(s) are absent from this checkout (not mounted in the sandbox, or deleted without git rm) and were left alone: they are still in the commit",
                self.absent
            ));
        }
        if self.edits > 0 {
            parts.push(format!("{} uncommitted change(s) the merge did not touch were left as they were", self.edits));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }
}

impl ThreadStatus {
    /// From the `key=value` lines `THREAD_END_SCRIPT status` prints.
    pub fn from_fields(f: &std::collections::HashMap<String, String>) -> Self {
        let get = |k: &str| f.get(k).map_or("", String::as_str);
        let num = |k: &str| get(k).parse().unwrap_or(0);
        ThreadStatus {
            worktree: get("worktree").into(),
            branch: get("branch").into(),
            base: get("base").into(),
            has_branch: get("has_branch") == "1",
            dirty: num("dirty"),
            ahead: num("ahead"),
        }
    }
}

/// The `key=value` lines a thread script printed, or its `error=` line.
pub fn script_fields(stdout: &str) -> Result<std::collections::HashMap<String, String>, String> {
    let fields: std::collections::HashMap<String, String> = stdout
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    match fields.get("error") {
        Some(err) => Err(err.clone()),
        None => Ok(fields),
    }
}

/// Write a memory into Claude Code's auto memory for the repo under `$1` —
/// what a shared plan's approved distillate becomes once it is merged.
///
/// Auto memory is per repository and shared by its worktrees: Claude Code
/// files it under `~/.claude/projects/<repo root, every character outside
/// [A-Za-z0-9] turned into '-'>/memory/`, keyed by the main checkout. The
/// memory goes in file `$2` (read from stdin, replaced if there), and one
/// pointer line `$3` goes in that folder's `MEMORY.md` index unless it is
/// there already. Arguments for `Sbx::exec` (after `--`), positional as in
/// `thread_run_args`.
pub fn memory_write_args(workspace: &str, file: &str, index_line: &str) -> Vec<String> {
    const SCRIPT: &str = concat!(unwrap_args!(), no_repo_fn!(), r#"
        [ -n "$1" ] && cd "$1" 2>/dev/null
        if ! common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null); then
            echo "error=$(sbxw_no_repo)"; exit 3
        fi
        root=$(dirname "$common")
        mem="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/$(printf '%s' "$root" | sed 's/[^A-Za-z0-9]/-/g')/memory"
        mkdir -p "$mem" || { echo "error=could not make $mem"; exit 4; }
        cat > "$mem/$2.sbxw-$$" && mv "$mem/$2.sbxw-$$" "$mem/$2" || { echo "error=could not write $mem/$2"; exit 5; }
        grep -qF "($2)" "$mem/MEMORY.md" 2>/dev/null || printf '%s
' "$3" >> "$mem/MEMORY.md"
        echo "path=$mem/$2"
        echo ok=1"#);
    let mut args = owned(&["bash", "-lc", SCRIPT, "sbxw-memory"]);
    args.extend([workspace, file, index_line].map(pos));
    args
}

/// Argv for a background session writing one codemap document:
/// `sbx exec <name> -- bash -lc <script> sbxw-codemap <agent-port> <name> <prompt>`.
///
/// Headless (`claude -p`): nothing here is a conversation. The one thing the
/// daemon has to say — where to report finishing — travels as
/// `$SBXW_CODEMAP_DONE`, a URL on the agent port carrying the sandbox's token
/// (the kit's command POSTs to it with a bare `curl`). The token is read
/// *inside* the sandbox, so it never appears in a host process's argv.
/// `bash -lc` picks up the persistent environment, and so `claude` on PATH.
pub fn codemap_run_args(name: &SandboxName, agent_port: &str, prompt: &str) -> Vec<String> {
    const SCRIPT: &str = "export SBXW_CODEMAP_DONE=\"http://host.docker.internal:$1/api/sandboxes/$2/codemap/done?token=$(cat \"$HOME/.sbxw/token\" 2>/dev/null)\"; \
        exec claude -p \"$3\" --permission-mode bypassPermissions";
    owned(&["exec", name.as_str(), "--", "bash", "-lc", SCRIPT, "sbxw-codemap", agent_port, name.as_str(), prompt])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sb() -> SandboxName {
        SandboxName::parse("myapp").unwrap()
    }
    fn th() -> ThreadName {
        ThreadName::parse("api-auth").unwrap()
    }

    #[test]
    fn a_thread_passes_everything_as_arguments_not_as_script() {
        let model = ModelId::parse("claude-opus-5-5").unwrap();
        let args = thread_run_args(&sb(), "/w s", &th(), "do it; rm -rf /", Some(&model), None, StartMode::New);
        let script = &args[6];
        assert!(!script.contains("rm -rf") && !script.contains("api-auth") && !script.contains("/w s"));
        assert_eq!(&args[7..], ["sbxw-thread", "=/w s", "=api-auth", "=do it; rm -rf /", "=claude-opus-5-5", "=", "=", "=new"]);
        assert_eq!(&args[..3], ["exec", "-it", "myapp"]);
        let no_model = thread_run_args(&sb(), "", &th(), "", None, None, StartMode::New);
        assert_eq!(no_model[11], "=");
    }

    #[test]
    fn a_same_worktree_session_starts_in_the_checkout_under_its_name() {
        let model = ModelId::parse("claude-opus-5-5").unwrap();
        let args = same_worktree_run_args(&sb(), "/w s", &th(), Some(&model));
        let script = &args[6];
        assert!(!script.contains("api-auth") && !script.contains("/w s"));
        assert!(!script.contains("worktree add") && !script.contains("--continue"), "no worktree, never main's conversation");
        assert!(script.contains(r#"export SBXW_THREAD="$2""#) && script.contains(r#"--name "$2""#));
        assert_eq!(&args[7..], ["sbxw-session", "=/w s", "=api-auth", "=claude-opus-5-5"]);
    }

    #[test]
    fn a_shared_plan_thread_opens_in_the_plan_worktree_with_its_env() {
        let team = Team { plan: "p-1".into(), worktree: ThreadName::parse("team-p-1").unwrap() };
        let args = thread_run_args(&sb(), "/w", &th(), "", None, Some(&team), StartMode::New);
        assert_eq!(&args[12..], ["=team-p-1", "=p-1", "=new"]);
        let script = &args[6];
        assert!(script.contains(r#"wt="$wtd/$key""#) && script.contains(r#"key="${5:-$2}""#));
        assert!(script.contains(r#"export SBXW_TEAM=shared SBXW_TEAM_PLAN="$6""#));
        let bash = thread_bash_args(&sb(), "/w", &th(), Some(&team));
        assert_eq!(&bash[7..], ["sbxw-thread-bash", "=/w", "=api-auth", "=team-p-1"]);
    }

    #[test]
    fn a_shared_plan_worktree_is_made_once_and_carries_the_env() {
        // The script itself, against a real repo: two threads land in one
        // worktree on one branch, with the team's env. `exec claude` becomes a
        // printout and `node -e` a no-op.
        let ok = |c: &mut std::process::Command| c.output().map(|o| o.status.success()).unwrap_or(false);
        if !ok(std::process::Command::new("git").arg("--version")) {
            return;
        }
        let d = std::env::temp_dir().join(format!("sbxw-team-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let r = d.join("repo");
        std::fs::create_dir(&r).unwrap();
        let git = |args: &[&str]| {
            assert!(ok(std::process::Command::new("git")
                .arg("-C")
                .arg(&r)
                .args(["-c", "user.email=t@t", "-c", "user.name=t", "-c", "init.defaultBranch=main"])
                .args(args)));
        };
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "one"]);
        let team = Team { plan: "p-1".into(), worktree: ThreadName::parse("team-p-1").unwrap() };
        for t in ["api", "web"] {
            let args = thread_run_args(&sb(), r.to_str().unwrap(), &ThreadName::parse(t).unwrap(), "go", None, Some(&team), StartMode::New);
            let script = args[6].replace("exec claude", "echo claude").replace("node -e", "true");
            let out = std::process::Command::new("bash")
                .env("HOME", &d)
                .args(["-c", &script])
                .args(&args[7..])
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(text.contains("worktree") && text.contains("branch sbxw/team-p-1"), "{text}");
        }
        let wt = r.join(".sbxw-worktrees/team-p-1");
        assert!(wt.is_dir());
        assert!(!r.join(".sbxw-worktrees/api").exists(), "no worktree of its own");
        let env = std::process::Command::new("bash")
            .args(["-c", &thread_run_args(&sb(), "", &th(), "", None, Some(&team), StartMode::Reopen)[6].replace("exec claude", "env #").replace("node -e", "true")])
            .args(["x", &format!("={}", r.display()), "=api-auth", "=", "=", "=team-p-1", "=p-1"])
            .env("HOME", &d)
            .output()
            .unwrap();
        let env = String::from_utf8_lossy(&env.stdout);
        assert!(env.contains("SBXW_TEAM=shared") && env.contains("SBXW_TEAM_PLAN=p-1") && env.contains("SBXW_THREAD=api-auth"), "{env}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn thread_script_arguments_arrive_unwrapped() {
        // The `=` prefix keeps `sbx exec` from refusing an empty element; the
        // script takes it off again.
        let out = std::process::Command::new("bash")
            .args(["-c", concat!(unwrap_args!(), r#"
printf '[%s]' "$@""#), "x", "=a", "=", "=b c"])
            .output();
        if let Ok(out) = out {
            assert_eq!(String::from_utf8_lossy(&out.stdout), "[a][][b c]");
        }
    }

    #[test]
    fn closing_and_merging_pass_everything_as_arguments() {
        let a = thread_end_args(&sb(), "/w", &th(), "close", true, false);
        assert_eq!(&a[7..], ["=/w", "=api-auth", "=close", "=1", "=0"]);
        let m = thread_merge_args(&sb(), "/w", &th());
        assert_eq!(&m[7..], ["=/w", "=api-auth"]);
        assert!(!THREAD_MERGE_SCRIPT.contains("api-auth"));
        let w = memory_write_args("/w", "team-p-1.md", "- [x](team-p-1.md) — y; z");
        assert_eq!(&w[4..], ["=/w", "=team-p-1.md", "=- [x](team-p-1.md) — y; z"]);
        assert!(!w[2].contains("team-p-1"));
    }

    /// A scratch git repository for the scripts' own logic, under the
    /// system's temp dir; `None` when git is not installed.
    struct Repo {
        dir: std::path::PathBuf,
        root: std::path::PathBuf,
    }

    impl Repo {
        fn new(tag: &str) -> Option<Repo> {
            let has_git = std::process::Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success());
            if !has_git {
                return None;
            }
            let dir = std::env::temp_dir().join(format!("sbxw-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let root = dir.join("repo");
            std::fs::create_dir_all(&root).unwrap();
            let r = Repo { dir, root };
            r.git(&["init", "-q"]);
            Some(r)
        }
        fn git(&self, args: &[&str]) -> String {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["-c", "user.email=t@t", "-c", "user.name=t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        }
        fn write(&self, path: &str, text: &str) {
            let p = self.root.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        fn rm(&self, path: &str) {
            let p = self.root.join(path);
            if p.is_dir() {
                std::fs::remove_dir_all(p).unwrap();
            } else {
                std::fs::remove_file(p).unwrap();
            }
        }
        /// main with files a, b, docs/c and src/d; branch sbxw/<t> changing `touch`.
        fn with_branch(&self, thread: &str, touch: &str) {
            for f in ["a", "b", "docs/c", "src/d"] {
                self.write(f, f);
            }
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", "one"]);
            self.git(&["checkout", "-q", "-b", &format!("sbxw/{thread}")]);
            self.write(touch, "changed on the branch");
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", "thread"]);
            self.git(&["checkout", "-q", "main"]);
        }
        /// Run a `key=value` script with its positional values, `=`-wrapped.
        fn run(&self, script: &str, args: &[&str]) -> (bool, String) {
            let out = std::process::Command::new("bash")
                .args(["-c", script, "x"])
                .args(args.iter().map(|a| format!("={a}")))
                .env("HOME", &self.dir)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned())
        }
        fn merge(&self, thread: &str) -> Result<MergeOutcome, String> {
            let (_, out) = self.run(THREAD_MERGE_SCRIPT, &[self.root.to_str().unwrap(), thread]);
            script_fields(&out).map(|f| MergeOutcome::from_fields(&f))
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_merge_leaves_files_absent_by_mount_alone_and_keeps_them_in_the_commit() {
        // The report: tracked files missing from the working tree (not
        // mounted), nothing staged; the branch touches other paths.
        let Some(r) = Repo::new("merge-absent") else { return };
        r.with_branch("api", "src/d");
        r.rm("a");
        r.rm("docs");
        assert_eq!(r.git(&["status", "--porcelain", "--untracked-files=no"]).lines().count(), 2);
        let m = r.merge("api").expect("an untouched absent file does not block");
        assert_eq!((m.absent, m.edits), (2, 0));
        assert!(m.note().unwrap().contains("2 tracked file(s) are absent"), "{:?}", m.note());
        let tree = r.git(&["ls-tree", "-r", "--name-only", "HEAD"]);
        assert_eq!(tree.lines().collect::<Vec<_>>(), ["a", "b", "docs/c", "src/d"], "nothing recorded as deleted");
        assert_eq!(r.git(&["show", "HEAD:src/d"]), "changed on the branch");
        assert!(!r.root.join("a").exists(), "and nothing written back where it was absent");
        assert_eq!(r.git(&["rev-list", "--parents", "-n1", "HEAD"]).split_whitespace().count(), 3, "a merge commit");
    }

    #[test]
    fn a_merge_touching_an_absent_file_is_refused_without_advising_a_commit() {
        let Some(r) = Repo::new("merge-absent-hit") else { return };
        r.with_branch("api", "docs/c");
        r.rm("docs");
        let head = r.git(&["rev-parse", "HEAD"]);
        let err = r.merge("api").unwrap_err();
        assert!(err.contains("missing from the main checkout") && err.contains("docs/c"), "{err}");
        assert!(err.contains("partly mounted") && err.contains("where the whole repository is present"), "{err}");
        assert!(err.contains("Do not commit these"), "{err}");
        assert!(!err.contains("commit or stash"), "{err}");
        assert_eq!(r.git(&["rev-parse", "HEAD"]), head, "nothing merged");
    }

    #[test]
    fn a_merge_is_refused_only_for_edits_to_the_paths_it_changes() {
        let Some(r) = Repo::new("merge-edits") else { return };
        r.with_branch("api", "src/d");
        r.write("b", "unfinished");
        let m = r.merge("api").expect("an edit elsewhere is left alone");
        assert_eq!((m.absent, m.edits), (0, 1));
        assert_eq!(std::fs::read_to_string(r.root.join("b")).unwrap(), "unfinished");
        assert_eq!(r.git(&["show", "HEAD:b"]), "b", "not in the merge commit");

        let Some(r) = Repo::new("merge-edits-hit") else { return };
        r.with_branch("api", "src/d");
        r.write("src/d", "mine");
        let err = r.merge("api").unwrap_err();
        assert!(err.contains("uncommitted changes to 1 file(s) this merge changes (src/d)") && err.contains("commit or stash those files"), "{err}");
    }

    #[test]
    fn a_merge_is_refused_with_staged_changes_or_mid_operation() {
        let Some(r) = Repo::new("merge-staged") else { return };
        r.with_branch("api", "src/d");
        r.write("b", "staged");
        r.git(&["add", "b"]);
        let err = r.merge("api").unwrap_err();
        assert!(err.contains("1 staged change(s) (b)") && err.contains("commit or unstage"), "{err}");

        let Some(r) = Repo::new("merge-mid") else { return };
        r.with_branch("api", "src/d");
        let head = r.git(&["rev-parse", "HEAD"]);
        std::fs::write(r.root.join(".git/MERGE_HEAD"), head).unwrap();
        let err = r.merge("api").unwrap_err();
        assert!(err.contains("in the middle of a merge") && err.contains("MERGE_HEAD"), "{err}");
    }

    #[test]
    fn a_conflict_is_aborted_and_absent_files_stay_absent() {
        let Some(r) = Repo::new("merge-conflict") else { return };
        r.with_branch("api", "b");
        r.write("b", "main's own");
        r.git(&["commit", "-q", "-am", "main"]);
        r.rm("a");
        let err = r.merge("api").unwrap_err();
        assert_eq!(err, "conflict in b");
        assert!(!r.root.join(".git/MERGE_HEAD").exists(), "aborted");
        assert!(!r.root.join("a").exists());
        assert_eq!(r.git(&["status", "--porcelain"]), " D a\n");
    }

    #[test]
    fn a_root_the_agent_cannot_write_is_a_reason_not_a_git_error() {
        let Some(r) = Repo::new("prepare-ro") else { return };
        r.with_branch("other", "a");
        let uid = std::process::Command::new("id").arg("-u").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        if uid.as_deref().ok() == Some("0") {
            return; // root writes anywhere: nothing to show
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&r.root, std::fs::Permissions::from_mode(0o555)).unwrap();
        let (ok, out) = r.run(THREAD_PREPARE_SCRIPT, &[r.root.to_str().unwrap(), "api", ""]);
        std::fs::set_permissions(&r.root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!ok);
        let err = script_fields(&out).unwrap_err();
        assert!(err.starts_with(&format!("cannot create {}/.sbxw-worktrees: {} is not writable by", r.root.display(), r.root.display())), "{err}");
        assert!(err.contains("give the sandbox the whole repository"), "{err}");
        assert!(!err.contains("leading directories"), "{err}");
        assert!(!r.git(&["branch", "--list"]).contains("sbxw/api"), "no branch left behind");

        let (ok, out) = r.run(THREAD_PREPARE_SCRIPT, &[r.root.to_str().unwrap(), "api", ""]);
        assert!(ok, "{out}");
        let f = script_fields(&out).unwrap();
        assert_eq!(f["worktree"], format!("{}/.sbxw-worktrees/api", r.root.display()));
        assert!(r.root.join(".sbxw-worktrees/api/a").exists());
        let exclude = std::fs::read_to_string(r.root.join(".git/info/exclude")).unwrap();
        assert!(exclude.lines().any(|l| l == "/.sbxw-worktrees/"), "excluded, not in .gitignore");
        assert!(r.git(&["status", "--porcelain"]).is_empty());
    }

    /// `thread_run_args`' script against a real repo, `claude` printed
    /// rather than run, with Claude Code's registry under the repo's HOME.
    fn run_thread(r: &Repo, prompt: &str, team: Option<&Team>, mode: StartMode) -> String {
        let args = thread_run_args(&sb(), r.root.to_str().unwrap(), &th(), prompt, None, team, mode);
        let script = args[6].replace("exec claude", "exec echo claude");
        let out = std::process::Command::new("bash")
            .args(["-c", &script])
            .args(&args[7..])
            .env("HOME", &r.dir)
            .env_remove("CLAUDE_CONFIG_DIR")
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn register(r: &Repo, pid: u32, session: &str, cwd: &str, started: u64) {
        let dir = r.dir.join(".claude/sessions");
        std::fs::create_dir_all(&dir).unwrap();
        let entry = serde_json::json!({ "pid": pid, "sessionId": session, "cwd": cwd, "name": "api-auth", "startedAt": started });
        std::fs::write(dir.join(format!("{pid}.json")), entry.to_string()).unwrap();
    }

    fn has_node() -> bool {
        std::process::Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success())
    }

    #[test]
    fn a_reopened_thread_resumes_its_conversation_by_id_or_starts_with_its_goal() {
        let Some(r) = Repo::new("thread-reopen") else { return };
        if !has_node() {
            return;
        }
        r.with_branch("other", "a");
        let first = run_thread(&r, "the briefing", None, StartMode::New);
        assert!(first.trim_end().ends_with("claude --dangerously-skip-permissions --name api-auth the briefing"), "{first}");
        let wt = format!("{}/.sbxw-worktrees/api-auth", r.root.display());

        // Reopened with nothing to resume: the goal, said as a restart.
        let again = run_thread(&r, "restart: the briefing", None, StartMode::Reopen);
        assert!(again.contains("no conversation of thread api-auth to resume: starting it again with its goal"), "{again}");
        assert!(again.trim_end().ends_with("--name api-auth restart: the briefing"), "{again}");

        // Its last session, gone, with a transcript: resumed by id, in a
        // shared worktree too (where --continue would pick anyone's).
        let sid = "0f1e2d3c-0000-4000-8000-000000000001";
        register(&r, 999_999_999, sid, &wt, 2);
        register(&r, 999_999_998, "0f1e2d3c-0000-4000-8000-000000000000", &wt, 1);
        let projects = r.dir.join(".claude/projects").join(wt.replace(|c: char| !c.is_ascii_alphanumeric(), "-"));
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::write(projects.join(format!("{sid}.jsonl")), "{}\n").unwrap();
        let resumed = run_thread(&r, "restart: the briefing", None, StartMode::Reopen);
        assert!(resumed.trim_end().ends_with(&format!("--name api-auth --resume {sid}")), "{resumed}");

        // A new plan's first start never resumes an old conversation.
        let fresh = run_thread(&r, "a new briefing", None, StartMode::New);
        assert!(fresh.trim_end().ends_with("--name api-auth a new briefing"), "{fresh}");
    }

    #[test]
    fn a_session_still_running_without_a_pane_is_stopped_not_doubled() {
        let Some(r) = Repo::new("thread-orphan") else { return };
        if !has_node() {
            return;
        }
        r.with_branch("other", "a");
        run_thread(&r, "the briefing", None, StartMode::New);
        let wt = format!("{}/.sbxw-worktrees/api-auth", r.root.display());
        // The session sbxw lost its terminal to: still running, as the thread.
        // `--name api-auth` on its command line, as `claude` has it.
        let orphan = || -> u32 {
            let out = std::process::Command::new("bash")
                .args(["-c", "SBXW_THREAD=api-auth bash -c 'sleep 60; :' claude --name api-auth >/dev/null 2>&1 & echo $!"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
        };
        let gone = |pid: u32| {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z")
        };
        let sid = "0f1e2d3c-0000-4000-8000-00000000000a";
        let pid = orphan();
        register(&r, pid, sid, &wt, 5);
        let out = run_thread(&r, "restart: the briefing", None, StartMode::Reopen);
        assert!(out.contains(&format!("already has a session running in this sandbox (pid {pid})")), "{out}");
        assert!(out.contains(&format!("--resume {sid} [sbxw] Your session was stopped and resumed here")), "its conversation goes on here: {out}");
        assert!(out.contains("still on disk, uncommitted") && out.contains("git status"), "{out}");
        assert!(gone(pid), "the first session was stopped");

        // The thread's name in the environment alone is not enough: not the
        // session sbxw started (no --name), so it is left alone.
        let out = std::process::Command::new("bash")
            .args(["-c", "SBXW_THREAD=api-auth bash -c 'sleep 60; :' >/dev/null 2>&1 & echo $!"])
            .output()
            .unwrap();
        let half: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        register(&r, half, "0f1e2d3c-0000-4000-8000-00000000000c", &wt, 5);
        let out = run_thread(&r, "restart: the briefing", None, StartMode::Reopen);
        assert!(!out.contains("already has a session"), "{out}");
        assert!(!gone(half));
        let _ = std::process::Command::new("kill").arg(half.to_string()).status();

        // A process that is not the thread (a recycled pid) is left alone.
        let other = std::process::Command::new("sleep").arg("60").spawn().unwrap();
        register(&r, other.id(), "0f1e2d3c-0000-4000-8000-00000000000b", &wt, 6);
        let out = run_thread(&r, "restart: the briefing", None, StartMode::Reopen);
        assert!(!out.contains("already has a session"), "{out}");
        assert!(!gone(other.id()));
        let mut other = other;
        let _ = other.kill();
        let _ = other.wait();

        // At a new plan's first start the stray session is stopped too, and
        // the thread starts fresh with its briefing.
        let pid = orphan();
        register(&r, pid, sid, &wt, 7);
        let out = run_thread(&r, "a new briefing", None, StartMode::New);
        assert!(out.contains("already has a session running"), "{out}");
        assert!(out.trim_end().ends_with("--name api-auth a new briefing"), "{out}");
        assert!(gone(pid));
    }

    #[test]
    fn script_output_is_read_as_fields_or_an_error() {
        let f = script_fields("worktree=/w/.sbxw-worktrees/a\nbranch=sbxw/a\ndirty=2\nhas_branch=1\nahead=3\n").unwrap();
        let st = ThreadStatus::from_fields(&f);
        assert_eq!((st.dirty, st.ahead, st.has_branch), (2, 3, true));
        assert_eq!(script_fields("error=conflict in a.rs\n").unwrap_err(), "conflict in a.rs");
    }

    #[test]
    fn a_codemap_session_carries_its_url_and_prompt_as_arguments() {
        let a = codemap_run_args(&sb(), "7682", "/codemap-lens a; b");
        assert_eq!(&a[6..], ["sbxw-codemap", "7682", "myapp", "/codemap-lens a; b"]);
        assert!(!a[5].contains("codemap-lens") && !a[5].contains("myapp"));
    }
}
