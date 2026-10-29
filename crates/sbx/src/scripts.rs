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
/// thread, with the repos found under it — a workspace is often a folder of
/// repos (`backend/`, `frontend/`), and the fix is to pick one of them
/// (`repo=` on the thread's socket). One line, no colour:
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

/// Argv for a *thread*: a second Claude session in `name`, working in a git
/// worktree of its own, `<repo>/.sbxw-worktrees/<thread>` on branch
/// `sbxw/<thread>`.
///
/// Two agents editing one working tree overwrite each other's files and mix
/// their changes into one diff nobody can review, so each thread gets a
/// checkout of its own. The worktree outlives the session: reopening the
/// thread lands back in it, on its branch, with its uncommitted work. Removing
/// one is left to a person — it can hold work nobody has committed.
///
/// `.sbxw-worktrees/` goes in the repository's `info/exclude`, not its
/// `.gitignore`: hiding sbxw's own bookkeeping from `git status` is not a
/// change to the project that anyone should have to commit.
///
/// `sbx exec` rather than `sbx run`: `run` attaches *the* agent, and a second
/// one would be that same session again. The permission flag is the one sbx
/// gives the agent it starts, so a thread is no more and no less trusted than
/// the pane beside it. `bash -lc` is a login shell, for `claude` on PATH (see
/// `codemap_run_args`).
///
/// A thread reopened without a prompt resumes its last conversation
/// (`claude --continue`) when it has one: its pane after the daemon or the
/// sandbox restarted, which ends every session sbxw held.
///
/// `SBXW_THREAD` names the thread to everything the session starts — the relay
/// MCP server reads it to know it serves a thread (and not the main session,
/// which is the one that can lead). `prompt`, when given, is the session's first
/// message — an approved plan's briefing.
///
/// `--name` makes the thread's name its address for Claude Code's own messaging
/// between sessions on one machine: the lead writes to `api-auth` with
/// `SendMessage`, and sbxw is not on that path. Without it the session is
/// listed under a name derived from its folder (`api-auth-5e`) or a title
/// Claude Code makes up from its first prompt, neither of which a lead can
/// predict.
///
/// The worktree is marked trusted in `.claude.json` before `claude` starts, as
/// `Tooling::trust_workspace` does for the workspace: Claude Code does not
/// extend a folder's trust to the folders under it, so every new worktree
/// would otherwise open on the trust dialog instead of on its briefing.
///
/// `model`, when given, is passed as `claude --model`, on a fresh start and on
/// `--continue` alike; empty leaves the sandbox's default
/// (`ANTHROPIC_DEFAULT_MODEL`) in charge.
///
/// `team`, for a thread of a shared plan (Team mode), is that plan: the
/// session opens in the plan's one worktree, `.sbxw-worktrees/team-<plan>` on
/// branch `sbxw/team-<plan>`, made by whichever thread starts first (under a
/// lock: they all start at once), with `SBXW_TEAM=shared` and
/// `SBXW_TEAM_PLAN=<plan>` set. Such a thread never `--continue`s: every
/// thread shares the cwd its transcripts are filed under, so "its last
/// conversation" could be another thread's.
///
/// Workspace, thread name, prompt, model and plan are positional (`$1`–`$6`),
/// never script text: the thread name comes from a WebSocket query parameter.
/// It is a `ThreadName`, and that is exactly why the
/// property is easy to lose — nothing downstream would notice.
pub fn thread_run_args(
    name: &SandboxName,
    workspace: &str,
    thread: &ThreadName,
    prompt: &str,
    model: Option<&ModelId>,
    team: Option<&Team>,
) -> Vec<String> {
    const SCRIPT: &str = concat!(unwrap_args!(), no_repo_fn!(), r#"
        [ -n "$1" ] && cd "$1" 2>/dev/null
        if ! root=$(git rev-parse --show-toplevel 2>/dev/null); then
            printf '\033[31m[sbxw] %s\033[0m\r\n' "$(sbxw_no_repo)"
            exit 1
        fi
        key="${5:-$2}"
        wt="$root/.sbxw-worktrees/$key"
        if [ ! -d "$wt" ] && [ -n "$5" ] && command -v flock >/dev/null; then
            # A shared plan's threads all start at once: one makes the worktree.
            exec 9>"$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)/sbxw-$key.lock"
            flock -w 60 9
        fi
        if [ ! -d "$wt" ]; then
            ex=$(git -C "$root" rev-parse --git-path info/exclude)
            case "$ex" in /*) ;; *) ex="$root/$ex" ;; esac
            mkdir -p "$(dirname "$ex")"
            grep -qxF '/.sbxw-worktrees/' "$ex" 2>/dev/null || printf '/.sbxw-worktrees/\n' >> "$ex"
            if git -C "$root" show-ref --verify --quiet "refs/heads/sbxw/$key"; then
                git -C "$root" worktree add "$wt" "sbxw/$key" || exit 1
            else
                git -C "$root" worktree add -b "sbxw/$key" "$wt" || exit 1
            fi
        fi
        exec 9>&-
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
        # Reopened with nothing new to say (its pane after a restart of the
        # daemon or the sandbox): pick its conversation back up, if it had one.
        # Claude Code files transcripts under the cwd with every character
        # outside [A-Za-z0-9] turned into '-'.
        tr="$HOME/.claude/projects/$(printf '%s' "$wt" | sed 's/[^A-Za-z0-9]/-/g')"
        if [ -z "$3" ] && [ -z "$5" ] && ls "$tr"/*.jsonl >/dev/null 2>&1; then
            printf '\033[2m[sbxw] resuming its last conversation\033[0m\r\n'
            exec claude --dangerously-skip-permissions --name "$2" ${4:+--model "$4"} --continue
        fi
        exec claude --dangerously-skip-permissions --name "$2" ${4:+--model "$4"} ${3:+"$3"}"#);
    let mut args = owned(&["exec", "-it", name.as_str(), "--", "bash", "-lc", SCRIPT, "sbxw-thread"]);
    let model = model.map(ModelId::as_str).unwrap_or("");
    let (worktree, plan) = team.map_or(("", ""), |t| (t.worktree.as_str(), t.plan.as_str()));
    args.extend([workspace, thread.as_str(), prompt, model, worktree, plan].map(pos));
    args
}

/// Argv for a session in the main checkout: another Claude beside the main
/// agent, in the workspace itself — no worktree, no branch, the same files.
/// Two agents in one checkout can overwrite each other's edits; that is what
/// the person chose it for (a second conversation about the same code), and
/// the menu that offers it says so.
///
/// Like a thread, it runs through `sbx exec` under `SBXW_THREAD` and
/// `--name`, which is how its hooks are filed under its name and not read as
/// the main agent's. It never `--continue`s: it shares the main agent's cwd,
/// so "its last conversation" would be the main agent's.
///
/// Workspace, name and model are positional, as in `thread_run_args`.
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

/// Where a thread stands, read before closing it: what closing would throw
/// away.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ThreadStatus {
    /// Its worktree, as the sandbox sees it.
    pub worktree: String,
    /// Its branch, `sbxw/<thread>`.
    pub branch: String,
    /// The branch the main checkout is on — what "merged" is measured against.
    pub base: String,
    /// Does the branch exist at all?
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
/// `discard` says otherwise — the browser has asked by then, but the daemon
/// does not trust a click it did not see.
///
/// Everything variable is positional, as in `thread_run_args`.
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
/// Refuses a main checkout with uncommitted changes to tracked files: a merge
/// into one mixes the person's unfinished work into the merge commit, or fails
/// halfway. Untracked files are left to git, which refuses only if the merge
/// would overwrite one. On a failed merge it runs `git merge --abort`, so the
/// checkout is as it was, and names the conflicting files.
///
/// Prints `key=value` lines like `THREAD_END_SCRIPT`: `commit=` on success,
/// `error=` otherwise. Everything variable is positional.
pub const THREAD_MERGE_SCRIPT: &str = concat!(unwrap_args!(), no_repo_fn!(), r#"
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
    n=$(git -C "$root" status --porcelain --untracked-files=no | wc -l | tr -d ' ')
    if [ "$n" -gt 0 ]; then
        echo "error=the main checkout of $root has $n uncommitted change(s); commit or stash them first"; exit 6
    fi
    if out=$(git -C "$root" merge --no-ff --no-edit "$br" 2>&1); then
        echo "base=$base"
        echo "commit=$(git -C "$root" rev-parse --short HEAD)"
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

impl ThreadStatus {
    /// From the `key=value` lines `THREAD_END_SCRIPT status` prints.
    pub fn from_fields(f: &std::collections::HashMap<String, String>) -> Self {
        let get = |k: &str| f.get(k).cloned().unwrap_or_default();
        let num = |k: &str| get(k).parse().unwrap_or(0);
        ThreadStatus {
            worktree: get("worktree"),
            branch: get("branch"),
            base: get("base"),
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
        SandboxName::parse("neos").unwrap()
    }
    fn th() -> ThreadName {
        ThreadName::parse("api-auth").unwrap()
    }

    #[test]
    fn a_thread_passes_everything_as_arguments_not_as_script() {
        let model = ModelId::parse("claude-opus-5-5").unwrap();
        let args = thread_run_args(&sb(), "/w s", &th(), "do it; rm -rf /", Some(&model), None);
        let script = &args[6];
        assert!(!script.contains("rm -rf") && !script.contains("api-auth") && !script.contains("/w s"));
        assert_eq!(&args[7..], ["sbxw-thread", "=/w s", "=api-auth", "=do it; rm -rf /", "=claude-opus-5-5", "=", "="]);
        assert_eq!(&args[..3], ["exec", "-it", "neos"]);
        let no_model = thread_run_args(&sb(), "", &th(), "", None, None);
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
        let args = thread_run_args(&sb(), "/w", &th(), "", None, Some(&team));
        assert_eq!(&args[12..], ["=team-p-1", "=p-1"]);
        let script = &args[6];
        assert!(script.contains(r#"wt="$root/.sbxw-worktrees/$key""#) && script.contains(r#"key="${5:-$2}""#));
        assert!(script.contains(r#"export SBXW_TEAM=shared SBXW_TEAM_PLAN="$6""#));
        let bash = thread_bash_args(&sb(), "/w", &th(), Some(&team));
        assert_eq!(&bash[7..], ["sbxw-thread-bash", "=/w", "=api-auth", "=team-p-1"]);
    }

    #[test]
    fn a_shared_plan_worktree_is_made_once_and_carries_the_env() {
        // The script itself, against a real repo: two threads land in one
        // worktree on one branch, with the team's env. `claude` and `node` are
        // stubbed by functions; `exec claude` becomes a printout.
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
            let args = thread_run_args(&sb(), r.to_str().unwrap(), &ThreadName::parse(t).unwrap(), "go", None, Some(&team));
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
            .args(["-c", &thread_run_args(&sb(), "", &th(), "", None, Some(&team))[6].replace("exec claude", "env #").replace("node -e", "true")])
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
        assert_eq!(&a[6..], ["sbxw-codemap", "7682", "neos", "/codemap-lens a; b"]);
        assert!(!a[5].contains("codemap-lens") && !a[5].contains("neos"));
    }
}
