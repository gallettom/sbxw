//! What sbxw installs inside a sandbox, as one Node script.
//!
//! Every `sbx exec` is a process spawn on the host and a hop into the VM, so
//! the in-sandbox half of a bring-up is collected here and run as a single
//! script piped to `node -`. Each piece promises three things:
//!
//! * **Merges, not overwrites.** `settings.json` and `.claude.json` are read
//!   once, mutated by every piece, written once; they also hold whatever the
//!   user configured in-session.
//! * **Best-effort, per piece.** A piece that throws has its changes rolled
//!   back and is reported by name; the others still run.
//! * **Idempotent.** Each hook removes its previous copy before adding itself;
//!   the memory section is fenced and replaced in place.
//!
//! New in the rebuild: the scripts talk to the daemon's **agent port**, and the
//! sandbox's **agent token** is installed at `~/.sbxw/token` (0600) for them to
//! present. The script is built here (pure); the daemon runs it.

pub const HOME: &str = "/home/agent";
pub const SETTINGS_PATH: &str = "/home/agent/.claude/settings.json";
pub const CLAUDE_JSON_PATH: &str = "/home/agent/.claude.json";
pub const ARTIFACT_HOOK_PATH: &str = "/home/agent/.sbxw/enforce-artifacts.js";
pub const STATUS_HOOK_PATH: &str = "/home/agent/.sbxw/status-hook.js";
pub const USAGE_STATUSLINE_PATH: &str = "/home/agent/.sbxw/usage-statusline.js";
pub const RELAY_TOOL_PATH: &str = "/home/agent/.sbxw/relay.js";
pub const RELAY_MCP_PATH: &str = "/home/agent/.sbxw/relay-mcp.js";
pub const TEAM_GUARD_PATH: &str = "/home/agent/.sbxw/team-guard.js";
/// The tools Team mode's guard checks before they run.
pub const TEAM_GUARD_MATCHER: &str = "Edit|Write|MultiEdit|NotebookEdit|Bash";
pub const TOKEN_PATH: &str = "/home/agent/.sbxw/token";
pub const RELAY_MCP_NAME: &str = "sbxw-relay";
pub const USER_MEMORY_PATH: &str = "/home/agent/.claude/CLAUDE.md";
pub const LEAD_COMMAND_PATH: &str = "/home/agent/.claude/commands/lead.md";
pub const CODEMAP_COMMAND_PATH: &str = "/home/agent/.claude/commands/codemap.md";
pub const CODEMAP_LENS_COMMAND_PATH: &str = "/home/agent/.claude/commands/codemap-lens.md";
pub const RELAY_DOC_BEGIN: &str = "<!-- sbxw:relay:begin -->";
pub const RELAY_DOC_END: &str = "<!-- sbxw:relay:end -->";

/// Claude Code lifecycle events forwarded to the daemon. (Permission prompts
/// arrive as `Notification`; structured questions as `PreToolUse`.)
pub const STATUS_HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
];

const ARTIFACT_HOOK: &str = include_str!("../../../assets/enforce-artifacts.js");
const STATUS_HOOK: &str = include_str!("../../../assets/status-hook.js");
const USAGE_STATUSLINE: &str = include_str!("../../../assets/usage-statusline.js");
const RELAY_TOOL: &str = include_str!("../../../assets/relay-tool.js");
const RELAY_MCP: &str = include_str!("../../../assets/relay-mcp.js");
const TEAM_GUARD: &str = include_str!("../../../assets/team-guard.js");
pub const LEAD_COMMAND: &str = include_str!("../../../assets/lead.md");

/// Names a failed piece is reported under.
pub mod piece {
    pub const TOKEN: &str = "token";
    pub const TRUST: &str = "trust";
    pub const ARTIFACT_HOOK: &str = "artifact-hook";
    pub const STATUS_HOOKS: &str = "status-hooks";
    pub const USAGE_STATUSLINE: &str = "usage-statusline";
    pub const RELAY: &str = "relay";
    pub const LEAD: &str = "lead";
    pub const STALE_MODEL: &str = "stale-model";
}

/// What the agent is told about the relay, in its user memory: the *trigger*,
/// not the manual. Short and unhedged — a brake ("use it sparingly") reads the
/// same whether or not the case in front of it is the one worth spending on;
/// restraint lives in the tool descriptions, read while deciding to call.
pub fn relay_doc() -> String {
    format!(
        "{RELAY_DOC_BEGIN}\n\
         ## When the answer isn't in this workspace\n\
         \n\
         Other sandboxes run on this machine, each with its own agent and its own\n\
         project, and you can ask them — a human routes the question and approves\n\
         the answer. Use the **`ask_other_sandbox`** tool (MCP server\n\
         `{RELAY_MCP_NAME}`) for this.\n\
         \n\
         The moment to reach for it is easy to miss, so name it explicitly: you\n\
         have searched, and what you need is in a repo, service or project that\n\
         is not mounted here. **Ask before you tell the user you cannot see it,\n\
         and before you offer to go looking elsewhere.** Finding the boundary of\n\
         your workspace is not the answer to a question — it is the point at\n\
         which to ask one.\n\
         \n\
         Same thing from a shell, if the tool is unavailable:\n\
         `node {RELAY_TOOL_PATH} ask \"your question\"` (then `wait <id>`).\n\
         \n\
         The question is read by a human and then by an agent that knows nothing\n\
         of this conversation, so make it self-contained. Never send secrets or\n\
         file contents you were not asked to share.\n\
         \n\
         ## When you can't see what you changed\n\
         \n\
         You cannot see this project's screen. When the work was visual — a\n\
         layout, a style, a component, anything whose correctness is a matter of\n\
         how it looks — **ask to be shown it** with the\n\
         **`ask_user_for_screenshot`** tool, before you call it done and before\n\
         a third round of \"is it better now?\".\n\
         \n\
         The moment is easy to miss because nothing is failing: the change is\n\
         made, the diff is right, and it still might look wrong. \"The code is\n\
         correct\" and \"it looks correct\" are different claims, and only one of\n\
         them is yours to make from here.\n\
         \n\
         From a shell: `node {RELAY_TOOL_PATH} shot \"what you need to see\"`.\n\
         \n\
         Say which screen, which state, which width — and if one picture will\n\
         not settle it, name every view you need in the same ask (\"at 375px\n\
         and at 1280px\"): they can attach several images to one reply, and\n\
         that is one interruption instead of three. The user may refuse — that\n\
         is an ordinary answer, not a setback: carry on, say what you changed\n\
         and what you expect it to look like, and let them correct you. Don't\n\
         ask twice for the same view, and don't ask at all for something you can\n\
         run and capture yourself.\n\
         {RELAY_DOC_END}\n"
    )
}

/// A JavaScript literal. JSON is a subset of JavaScript, so a string encoded
/// this way cannot break out of the script it is spliced into.
fn js<T: serde::Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).expect("strings and lists always serialise")
}

/// The line the script reports failures after, alone, so nothing else on
/// stdout can be mistaken for it.
const REPORT: &str = "SBXW-TOOLING ";

const PRELUDE: &str = "\
const fs=require('fs'),path=require('path');\
let docs={};const touched={};const failed=[];let who='';\
const put=(p,s,mode)=>{fs.mkdirSync(path.dirname(p),{recursive:true});fs.writeFileSync(p,s,mode?{mode}:undefined);if(mode)fs.chmodSync(p,mode)};\
const json=p=>{(touched[p]=touched[p]||new Set()).add(who);\
if(!(p in docs)){try{docs[p]=JSON.parse(fs.readFileSync(p,'utf8'))}catch(e){docs[p]={}}}return docs[p]};\
const msg=e=>String((e&&e.message)||e);\
const piece=(name,f)=>{who=name;const snap=JSON.stringify(docs);\
try{f()}catch(e){docs=JSON.parse(snap);failed.push([name,msg(e)])}};";

const EPILOGUE: &str = "\
for(const p of Object.keys(docs)){try{put(p,JSON.stringify(docs[p]))}\
catch(e){for(const w of touched[p]||[])failed.push([w,msg(e)])}}";

#[derive(Default, Clone)]
pub struct Tooling {
    pieces: Vec<(&'static str, String)>,
}

impl Tooling {
    pub fn new() -> Self {
        Self::default()
    }

    fn piece(mut self, name: &'static str, body: String) -> Self {
        self.pieces.push((name, body));
        self
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.pieces.iter().map(|(n, _)| *n).collect()
    }

    /// The sandbox's agent token, owner-only.
    pub fn agent_token(self, token: &str) -> Self {
        let body = format!("put({},{},0o600);", js(TOKEN_PATH), js(&format!("{token}\n")));
        self.piece(piece::TOKEN, body)
    }

    /// Pre-trust the workspace, or Claude Code shows the trust dialog and
    /// ignores `.claude/settings.local.json`'s permissions until accepted.
    pub fn trust_workspace(self, workspace: &str) -> Self {
        let body = format!(
            "const d=json({p});const w={w};d.projects=d.projects||{{}};\
             d.projects[w]=Object.assign({{}},d.projects[w],{{hasTrustDialogAccepted:true}});",
            p = js(CLAUDE_JSON_PATH),
            w = js(workspace),
        );
        self.piece(piece::TRUST, body)
    }

    /// A user-level `PreToolUse` hook keeping new deliverables in `.sbxw-artifacts/`.
    pub fn artifact_hook(self) -> Self {
        let body = format!(
            "const hookPath={path};put(hookPath,{script});const d=json({settings});\
             d.hooks=d.hooks||{{}};\
             d.hooks.PreToolUse=(d.hooks.PreToolUse||[]).filter(e=>!(e.hooks||[]).some(h=>(h.args||[]).includes(hookPath)));\
             d.hooks.PreToolUse.push({{matcher:'Write',hooks:[{{type:'command',command:'node',args:[hookPath]}}]}});",
            path = js(ARTIFACT_HOOK_PATH),
            script = js(ARTIFACT_HOOK),
            settings = js(SETTINGS_PATH),
        );
        self.piece(piece::ARTIFACT_HOOK, body)
    }

    /// Hooks that POST every lifecycle event to the daemon's agent port.
    pub fn status_hooks(self, agent_port: &str) -> Self {
        let body = format!(
            "const hookPath={path};const events={events};put(hookPath,{script});const d=json({settings});\
             d.hooks=d.hooks||{{}};\
             for(const ev of events){{d.hooks[ev]=(d.hooks[ev]||[]).filter(e=>!(e.hooks||[]).some(h=>(h.args||[]).includes(hookPath)));\
             d.hooks[ev].push({{hooks:[{{type:'command',command:'node',args:[hookPath]}}]}});}}",
            path = js(STATUS_HOOK_PATH),
            events = js(STATUS_HOOK_EVENTS),
            script = js(&STATUS_HOOK.replace("__PORT__", agent_port)),
            settings = js(SETTINGS_PATH),
        );
        self.piece(piece::STATUS_HOOKS, body)
    }

    /// The usage `statusLine` — the user's own one is kept.
    pub fn usage_statusline(self, agent_port: &str) -> Self {
        let body = format!(
            "put({path},{script});const cmd={cmd};const d=json({settings});\
             if(!d.statusLine||((d.statusLine.command||'').includes('usage-statusline.js'))){{d.statusLine={{type:'command',command:cmd}};}}",
            path = js(USAGE_STATUSLINE_PATH),
            script = js(&USAGE_STATUSLINE.replace("__PORT__", agent_port)),
            cmd = js(&format!("node {USAGE_STATUSLINE_PATH}")),
            settings = js(SETTINGS_PATH),
        );
        self.piece(piece::USAGE_STATUSLINE, body)
    }

    /// The relay: CLI, MCP server (user scope) and the fenced memory section;
    /// and Team mode's guard, a hook beside it that uses the relay to reach the
    /// board and does nothing outside a shared plan's threads. Installed here
    /// rather than with the status hooks so that Lead refreshes it too.
    pub fn relay(self, agent_port: &str) -> Self {
        let body = format!(
            "put({tool_path},{tool});put({mcp_path},{mcp});const d=json({claude_json});\
             d.mcpServers=d.mcpServers||{{}};\
             d.mcpServers[{name}]={{type:'stdio',command:'node',args:[{mcp_path}],env:{{}}}};\
             const p={memory},begin={begin},end={end},doc={doc};\
             let cur='';try{{cur=fs.readFileSync(p,'utf8')}}catch(e){{}}\
             const from=cur.indexOf(begin),to=cur.indexOf(end);\
             let rest=(from>=0&&to>from)?cur.slice(0,from)+cur.slice(to+end.length):cur;\
             rest=rest.replace(/\\n{{3,}}/g,'\\n\\n').trim();\
             put(p,(rest?rest+'\\n\\n':'')+doc);\
             const guard={guard_path};put(guard,{guard});const st=json({settings});st.hooks=st.hooks||{{}};\
             for(const [ev,matcher] of [['PreToolUse',{matcher}],['PostToolUse',undefined]]){{\
             st.hooks[ev]=(st.hooks[ev]||[]).filter(e=>!(e.hooks||[]).some(h=>(h.args||[]).includes(guard)));\
             const entry={{hooks:[{{type:'command',command:'node',args:[guard]}}]}};if(matcher)entry.matcher=matcher;\
             st.hooks[ev].push(entry);}}",
            tool_path = js(RELAY_TOOL_PATH),
            tool = js(&RELAY_TOOL.replace("__PORT__", agent_port)),
            mcp_path = js(RELAY_MCP_PATH),
            mcp = js(RELAY_MCP),
            claude_json = js(CLAUDE_JSON_PATH),
            name = js(RELAY_MCP_NAME),
            memory = js(USER_MEMORY_PATH),
            begin = js(RELAY_DOC_BEGIN),
            end = js(RELAY_DOC_END),
            doc = js(&relay_doc()),
            guard_path = js(TEAM_GUARD_PATH),
            guard = js(TEAM_GUARD),
            settings = js(SETTINGS_PATH),
            matcher = js(TEAM_GUARD_MATCHER),
        );
        self.piece(piece::RELAY, body)
    }

    /// `/lead`, as a user-level command (not `~/.claude/skills`, which is
    /// sbx's shared store on the host).
    pub fn lead_command(self) -> Self {
        let body = format!("put({},{});", js(LEAD_COMMAND_PATH), js(LEAD_COMMAND));
        self.piece(piece::LEAD, body)
    }

    /// Remove a settings `model` key — only when it is still sbxw's own
    /// leftover, since a settings `model` outranks `ANTHROPIC_DEFAULT_MODEL`
    /// and a differing one is somebody's deliberate `/model`.
    pub fn drop_stale_model(self, written_by_sbxw: &str) -> Self {
        let body = format!(
            "const d=json({});if(d.model==={}){{delete d.model;}}",
            js(SETTINGS_PATH),
            js(written_by_sbxw)
        );
        self.piece(piece::STALE_MODEL, body)
    }

    /// The whole batch as one Node script.
    pub fn script(&self) -> String {
        let mut s = String::from(PRELUDE);
        for (name, body) in &self.pieces {
            s.push_str(&format!("piece({},()=>{{{body}}});", js(name)));
        }
        s.push_str(EPILOGUE);
        s.push_str(&format!("console.log({}+JSON.stringify(failed));", js(REPORT)));
        s
    }

    /// The failures the script reported, by piece. `None` without a report
    /// line: the script did not get as far as reporting.
    pub fn parse_report(&self, stdout: &str) -> Option<ToolingOutcome> {
        let line = stdout.lines().rev().find_map(|l| l.strip_prefix(REPORT))?;
        let raw: Vec<(String, String)> = serde_json::from_str(line).ok()?;
        let mut failed: Vec<(&'static str, String)> = Vec::new();
        for (name, err) in raw {
            let Some(&(piece, _)) = self.pieces.iter().find(|(p, _)| *p == name) else {
                continue;
            };
            if !failed.iter().any(|(p, _)| *p == piece) {
                failed.push((piece, err));
            }
        }
        Some(ToolingOutcome { failed })
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ToolingOutcome {
    pub failed: Vec<(&'static str, String)>,
}

impl ToolingOutcome {
    pub fn error(&self, piece: &str) -> Option<&str> {
        self.failed.iter().find(|(p, _)| *p == piece).map(|(_, e)| e.as_str())
    }
}

/// What each piece costs when it fails, for the bring-up's warnings.
pub fn consequence(piece: &str) -> &'static str {
    match piece {
        piece::TOKEN => "could not install the agent token (hooks and the relay will be refused)",
        piece::TRUST => "could not pre-trust the workspace (accept the trust dialog manually)",
        piece::ARTIFACT_HOOK => "could not install the artifacts hook",
        piece::STATUS_HOOKS => "could not install the status hooks",
        piece::USAGE_STATUSLINE => "could not install the usage statusLine",
        piece::RELAY => "could not install the cross-sandbox relay",
        piece::LEAD => "could not install /lead",
        piece::STALE_MODEL => "could not clear the old settings.json model key",
        _ => "a tooling step failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn run_node(script: &str) -> Option<String> {
        let mut child = Command::new("node")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        child.stdin.take()?.write_all(script.as_bytes()).ok()?;
        let out = child.wait_with_output().ok()?;
        assert!(out.status.success());
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    #[test]
    fn the_batch_merges_is_idempotent_and_installs_the_token() {
        if Command::new("node").arg("--version").output().is_err() {
            eprintln!("node not found — skipping");
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude")).unwrap();
        std::fs::write(
            h.join(".claude/settings.json"),
            r#"{"model":"m1","statusLine":{"type":"command","command":"mine"},
                "hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"x"}]}]}}"#,
        )
        .unwrap();
        std::fs::write(h.join(".claude.json"), r#"{"onboarded":true}"#).unwrap();
        std::fs::write(h.join(".claude/CLAUDE.md"), "my own rule\n").unwrap();

        let batch = Tooling::new()
            .agent_token("tok")
            .trust_workspace("/work")
            .artifact_hook()
            .status_hooks("7682")
            .usage_statusline("7682")
            .relay("7682")
            .lead_command()
            .drop_stale_model("m1");
        let script = batch.script().replace("/home/agent/", &format!("{}/", h.display()));
        let read = |p: &str| std::fs::read_to_string(h.join(p)).unwrap();
        let out = run_node(&script).unwrap();
        assert_eq!(batch.parse_report(&out).unwrap(), ToolingOutcome::default(), "{out}");
        let first = (read(".claude/settings.json"), read(".claude.json"), read(".claude/CLAUDE.md"));
        run_node(&script).unwrap();
        assert_eq!(first, (read(".claude/settings.json"), read(".claude.json"), read(".claude/CLAUDE.md")));

        let s: serde_json::Value = serde_json::from_str(&first.0).unwrap();
        assert!(s.get("model").is_none());
        assert_eq!(s["statusLine"]["command"], "mine");
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 4, "the user's, artifacts, status, team guard");
        assert_eq!(s["hooks"]["PostToolUse"].as_array().unwrap().len(), 2, "status, team guard");
        assert!(read(".sbxw/team-guard.js").contains("SBXW_TEAM"));
        let c: serde_json::Value = serde_json::from_str(&first.1).unwrap();
        assert_eq!(c["projects"]["/work"]["hasTrustDialogAccepted"], true);
        assert_eq!(c["mcpServers"][RELAY_MCP_NAME]["command"], "node");
        assert!(first.2.starts_with("my own rule\n\n"));
        assert_eq!(read(".sbxw/token"), "tok\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(h.join(".sbxw/token")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let hook = read(".sbxw/status-hook.js");
        assert!(!hook.contains("__PORT__") && hook.contains("7682") && hook.contains("Bearer"));
        assert!(read(".claude/commands/lead.md").len() > 100);
    }

    /// The guard's exit code and stderr for one hook call in `cwd`.
    fn guard(dir: &std::path::Path, env_team: Option<&str>, input: serde_json::Value) -> (i32, String) {
        let script = dir.join("team-guard.js");
        std::fs::write(&script, TEAM_GUARD).unwrap();
        let mut cmd = Command::new("node");
        cmd.arg(&script).current_dir(dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.env_remove("SBXW_TEAM").env("SBXW_THREAD", "api");
        if let Some(t) = env_team {
            cmd.env("SBXW_TEAM", t);
        }
        let mut child = cmd.spawn().unwrap();
        child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stderr).into_owned())
    }

    #[test]
    fn the_team_guard_refuses_shared_index_git_and_sleeps_outside_team_mode() {
        let have = |c: &str| Command::new(c).arg("--version").output().is_ok();
        if !have("node") || !have("git") {
            eprintln!("node or git not found — skipping");
            return;
        }
        let repo = tempfile::tempdir().unwrap();
        let d = repo.path();
        assert!(Command::new("git").arg("init").arg("-q").current_dir(d).status().unwrap().success());
        let bash = |cmd: &str| {
            serde_json::json!({
                "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_use_id": format!("t{}", cmd.len()),
                "cwd": d, "tool_input": { "command": cmd },
            })
        };
        for refused in ["git add -A", "git add . && git commit -m x", "git commit -am x", "git stash", "git commit -m x"] {
            let (code, err) = guard(d, Some("shared"), bash(refused));
            assert_eq!(code, 2, "{refused}");
            assert!(err.contains("git"), "{refused}: {err}");
        }
        assert_eq!(guard(d, None, bash("git stash")).0, 0, "inactive outside a shared plan");
        assert_eq!(guard(d, Some("split"), bash("git stash")).0, 0);
        // Allowed (the daemon is unreachable here, so claims fail open).
        let commit = "git add a.ts && git commit -m x -- a.ts";
        assert_eq!(guard(d, Some("shared"), bash(commit)).0, 0);
        assert_eq!(guard(d, Some("shared"), bash("sed -i s/a/b/ a.ts > out.txt")).0, 0);
        let lock = d.join(".git/sbxw-commit.lock");
        assert!(lock.exists(), "the commit took the lock");
        let post = serde_json::json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_use_id": format!("t{}", commit.len()),
            "cwd": d, "tool_input": { "command": commit },
        });
        assert_eq!(guard(d, Some("shared"), post).0, 0);
        assert!(!lock.exists(), "and dropped it after");
    }

    #[test]
    fn a_report_names_the_pieces_that_failed() {
        let t = Tooling::new().trust_workspace("/w").relay("1");
        let out = "noise\nSBXW-TOOLING [[\"relay\",\"EACCES\"],[\"relay\",\"again\"],[\"nope\",\"x\"]]\n";
        let r = t.parse_report(out).unwrap();
        assert_eq!(r.failed, vec![(piece::RELAY, "EACCES".to_string())]);
        assert!(t.parse_report("no report").is_none());
    }
}
