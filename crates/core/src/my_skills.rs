//! The person's own skills: ones they made in a sandbox and chose to keep, so
//! every sandbox created afterwards has them.
//!
//! A kept skill is a folder under `~/.sbxw/skills/<name>/` on the host, copied
//! out of a sandbox on the person's say-so (the web UI's Skills dialog, on the
//! UI listener only). They reach sandboxes as one generated kit,
//! [`kit_spec`], which writes each file under `/home/agent/.sbxw/my-skills/`
//! and a thin `~/.claude/commands/<name>.md` pointing at its `SKILL.md` — the
//! arrangement the bundled kits use, because `~/.claude/skills` in a sandbox is
//! a mount of sbx's shared skill store.
//!
//! What a sandbox wrote is data on the host, never something it runs: the
//! files are not executable there, and the host's own `~/.claude/skills` is
//! never touched — a skill written by an agent in a sandbox must not become an
//! instruction to the person's host Claude Code.

use serde::{Deserialize, Serialize};

/// The generated kit's `name:`, also its directory under `state/kits/`.
pub const KIT_NAME: &str = "sbxw-my-skills";

/// Where a kept skill's files land inside a sandbox.
pub const SANDBOX_ROOT: &str = "/home/agent/.sbxw/my-skills";

/// The manifest sbxw keeps beside a skill's files on the host.
pub const MANIFEST: &str = ".sbxw-skill.json";

/// A skill bigger than this is not a skill but a project: refuse it.
pub const MAX_BYTES: usize = 512 * 1024;
pub const MAX_FILES: usize = 200;

/// Where a skill was read from inside its sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// `<workspace>/.claude/skills/<name>` — the project's own.
    Workspace,
    /// `~/.claude/skills/<name>`, when it is not sbx's store mount.
    Home,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Workspace => "workspace",
            Origin::Home => "home",
        }
    }
}

/// What sbxw records about a kept skill, beside its files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The sandbox it was copied from.
    pub sandbox: String,
    /// Its path there.
    pub path: String,
    /// When it was kept, in seconds since the epoch.
    pub saved: u64,
    /// Files that were executable in the sandbox, relative to the skill: they
    /// are executable again in the next sandboxes, never on the host.
    #[serde(default)]
    pub executable: Vec<String>,
}

/// One file of a skill, relative to its folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub executable: bool,
}

/// A skill as the kit renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub files: Vec<File>,
}

/// A skill's name is a folder name on the host, a path in the sandbox and a
/// slash command: lowercase letters, digits, `-` and `_`, as Claude Code's own.
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

/// A file's path inside a skill: relative, no `..`, no empty or dot segment,
/// nothing a YAML key or a shell would read twice.
pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 200
        && path != MANIFEST
        && path.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg.chars().all(|c| c.is_ascii_alphanumeric() || "._-+@ ".contains(c))
        })
}

/// Check a skill read out of a sandbox before anything is written: the
/// sandbox's own script applied the same limits, but its word is not taken.
pub fn check(name: &str, files: &[File]) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("'{name}' is not a skill name sbxw keeps: lowercase letters, digits, - and _"));
    }
    if files.len() > MAX_FILES {
        return Err(format!("{} files — a skill sbxw keeps has at most {MAX_FILES}", files.len()));
    }
    let total: usize = files.iter().map(|f| f.content.len()).sum();
    if total > MAX_BYTES {
        return Err(format!("{} KB — a skill sbxw keeps is at most {} KB", total / 1024, MAX_BYTES / 1024));
    }
    if let Some(bad) = files.iter().find(|f| !valid_path(&f.path)) {
        return Err(format!("file name '{}' is not kept (letters, digits, . _ - + @ and spaces only)", bad.path));
    }
    if !files.iter().any(|f| f.path == "SKILL.md") {
        return Err("no SKILL.md at the top of the folder".into());
    }
    Ok(())
}

/// The `description:` of a `SKILL.md`'s front matter, on one line.
pub fn description(skill_md: &str) -> Option<String> {
    let rest = skill_md.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let mut lines = rest[..end].lines().peekable();
    while let Some(line) = lines.next() {
        let Some(v) = line.strip_prefix("description:") else { continue };
        let v = v.trim();
        let mut text = if v == ">" || v == "|" || v == ">-" || v == "|-" { String::new() } else { v.to_string() };
        // A folded or continued value: the indented lines that follow.
        while let Some(next) = lines.peek().filter(|l| l.starts_with(' ') || l.starts_with('\t')) {
            text.push(' ');
            text.push_str(next.trim());
            lines.next();
        }
        let text = text.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_string();
        return (!text.is_empty()).then_some(text);
    }
    None
}

/// The thin command that makes `/<name>` read the kept skill.
fn command(name: &str, desc: Option<&str>) -> String {
    let dir = format!("{SANDBOX_ROOT}/{name}");
    let head = desc.map(|d| format!("---\ndescription: {}\n---\n\n", serde_json::to_string(d).unwrap_or_default())).unwrap_or_default();
    format!(
        "{head}Request: $ARGUMENTS\n\n\
         `{name}` is one of your own skills, kept by sbxw and installed in every new sandbox,\n\
         at `{dir}`. **Read `{dir}/SKILL.md` now and follow it** for this request.\n\
         Relative paths in it resolve against `{dir}/`; your working directory stays the project's.\n\
         To change the skill, edit a copy in the project's `.claude/skills/{name}/` and keep it\n\
         again from sbxw's Skills dialog: files under `{dir}` are rewritten when the sandbox is created.\n"
    )
}

/// The kit that installs every kept skill. Content is a JSON string, which is
/// a YAML double-quoted scalar: newlines and quotes survive, nothing folds.
pub fn kit_spec(skills: &[Skill]) -> String {
    let mut skills: Vec<&Skill> = skills.iter().collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    let mut out = format!(
        "schemaVersion: \"2\"\nkind: mixin\nname: {KIT_NAME}\n\
         description: Skills you kept from a sandbox with sbxw ({})\n",
        names.join(", ")
    );
    if skills.is_empty() {
        return out;
    }
    out.push_str("\nsetup:\n  files:\n");
    let mut file = |path: &str, mode: &str, content: &str| {
        let content = serde_json::to_string(content).unwrap_or_default();
        out.push_str(&format!("    - path: {}\n      mode: \"{mode}\"\n      content: {content}\n", serde_json::to_string(path).unwrap_or_default()));
    };
    for s in skills {
        let mut files: Vec<&File> = s.files.iter().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let desc = files.iter().find(|f| f.path == "SKILL.md").and_then(|f| description(&f.content));
        for f in files {
            file(&format!("{SANDBOX_ROOT}/{}/{}", s.name, f.path), if f.executable { "0755" } else { "0644" }, &f.content);
        }
        file(&format!("/home/agent/.claude/commands/{}.md", s.name), "0644", &command(&s.name, desc.as_deref()));
    }
    out
}

/// The node script that lists a sandbox's skills, or exports one. Argv:
/// `node - <mode> <workspace> [<origin> <name>]`, values behind `=` (`sbx
/// exec` refuses an empty element). It prints one JSON line.
pub const SANDBOX_SCRIPT: &str = r#"
const fs = require('fs'), path = require('path');
const [mode, ws, origin, name] = process.argv.slice(2).map(a => a.replace(/^=/, ''));
const HOME = '/home/agent/.claude/skills';
const MAX_BYTES = 512 * 1024, MAX_FILES = 200;
const out = v => process.stdout.write(JSON.stringify(v) + '\n');
// sbx mounts its shared store on ~/.claude/skills: what is there is kept
// already, by sbx, so it is not offered.
function isMount(p) {
  try {
    return fs.readFileSync('/proc/self/mountinfo', 'utf8').split('\n').some(l => l.split(' ')[4] === p);
  } catch { return false; }
}
const roots = { workspace: ws ? path.join(ws, '.claude/skills') : '', home: isMount(HOME) ? '' : HOME };
function walk(dir, rel, acc) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    if (e.isSymbolicLink()) { acc.skipped.push(rel + e.name + ' (link)'); continue; }
    const p = path.join(dir, e.name);
    if (e.isDirectory()) {
      if (e.name === 'node_modules' || e.name === '.git') { acc.skipped.push(rel + e.name + '/'); continue; }
      walk(p, rel + e.name + '/', acc);
    } else if (e.isFile()) {
      const st = fs.statSync(p);
      acc.count++; acc.bytes += st.size;
      if (acc.content && acc.count <= MAX_FILES && acc.bytes <= MAX_BYTES) {
        let text;
        try { text = new TextDecoder('utf-8', { fatal: true }).decode(fs.readFileSync(p)); }
        catch { acc.skipped.push(rel + e.name + ' (not text)'); acc.count--; acc.bytes -= st.size; continue; }
        acc.files.push({ path: rel + e.name, content: text, executable: (st.mode & 0o111) !== 0 });
      }
    }
  }
}
function describe(dir) {
  try {
    const md = fs.readFileSync(path.join(dir, 'SKILL.md'), 'utf8');
    const m = md.match(/^---\n[\s\S]*?^description:\s*(.*)$/m);
    return m ? m[1].replace(/^["'>|-]+|["']$/g, '').trim() : '';
  } catch { return null; }
}
if (mode === 'list') {
  const skills = [];
  for (const [o, root] of Object.entries(roots)) {
    if (!root) continue;
    let entries = [];
    try { entries = fs.readdirSync(root, { withFileTypes: true }); } catch { continue; }
    for (const e of entries) {
      if (!e.isDirectory()) continue;
      const dir = path.join(root, e.name);
      const description = describe(dir);
      if (description === null) continue;
      const acc = { count: 0, bytes: 0, skipped: [], files: [], content: false };
      try { walk(dir, '', acc); } catch {}
      skills.push({ name: e.name, origin: o, path: dir, description, files: acc.count, bytes: acc.bytes });
    }
  }
  out({ ok: true, skills });
} else if (mode === 'export') {
  const root = roots[origin];
  if (!root || !name || name.includes('/') || name.startsWith('.')) { out({ ok: false, error: 'no such skill' }); process.exit(0); }
  const dir = path.join(root, name);
  const acc = { count: 0, bytes: 0, skipped: [], files: [], content: true };
  try { walk(dir, '', acc); } catch (e) { out({ ok: false, error: e.message }); process.exit(0); }
  if (acc.count > MAX_FILES || acc.bytes > MAX_BYTES) {
    out({ ok: false, error: `${acc.count} files, ${Math.round(acc.bytes / 1024)} KB — a skill sbxw keeps is at most ${MAX_FILES} files and ${MAX_BYTES / 1024} KB` });
  } else {
    out({ ok: true, path: dir, files: acc.files, skipped: acc.skipped });
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, content: &str) -> File {
        File { path: path.into(), content: content.into(), executable: false }
    }

    #[test]
    fn names_and_paths_are_narrow() {
        assert!(valid_name("my-skill_2"));
        for bad in ["", "My", "-x", "a/b", "..", "a b", &"x".repeat(65)] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert!(valid_path("SKILL.md") && valid_path("scripts/run.sh") && valid_path("refs/a b.md"));
        for bad in ["", "/etc/x", "../x", "a/../b", "a//b", "./a", "a\"b", "a\nb", MANIFEST] {
            assert!(!valid_path(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_skill_needs_its_skill_md_and_stays_small() {
        assert!(check("x", &[f("SKILL.md", "")]).is_ok());
        assert!(check("x", &[f("README.md", "")]).unwrap_err().contains("SKILL.md"));
        assert!(check("x", &[f("SKILL.md", ""), f("../evil", "")]).is_err());
        assert!(check("x", &[f("SKILL.md", &"a".repeat(MAX_BYTES + 1))]).unwrap_err().contains("KB"));
    }

    #[test]
    fn the_description_is_read_plain_or_folded() {
        assert_eq!(description("---\nname: a\ndescription: Does a thing.\n---\nbody").as_deref(), Some("Does a thing."));
        assert_eq!(description("---\ndescription: >\n  Two\n  lines\nother: x\n---\n").as_deref(), Some("Two lines"));
        assert_eq!(description("---\ndescription: \"quoted\"\n---\n").as_deref(), Some("quoted"));
        assert_eq!(description("no front matter"), None);
    }

    #[test]
    fn the_kit_carries_every_file_intact_and_a_command_each() {
        let tricky = "line 1\n  indented: 'quote' \"double\"\n\ttab \\ backslash\n";
        let skills = vec![
            Skill { name: "zeta".into(), files: vec![f("SKILL.md", "---\ndescription: Z skill\n---\n")] },
            Skill {
                name: "alpha".into(),
                files: vec![f("SKILL.md", tricky), File { path: "bin/run.sh".into(), content: "#!/bin/sh\n".into(), executable: true }],
            },
        ];
        let spec = kit_spec(&skills);
        let doc: serde_norway::Value = serde_norway::from_str(&spec).unwrap();
        assert_eq!(doc["name"].as_str(), Some(KIT_NAME));
        let files = doc["setup"]["files"].as_sequence().unwrap();
        let get = |p: &str| files.iter().find(|x| x["path"].as_str() == Some(p)).unwrap_or_else(|| panic!("{p}"));
        assert_eq!(get("/home/agent/.sbxw/my-skills/alpha/SKILL.md")["content"].as_str(), Some(tricky));
        assert_eq!(get("/home/agent/.sbxw/my-skills/alpha/bin/run.sh")["mode"].as_str(), Some("0755"));
        let cmd = get("/home/agent/.claude/commands/zeta.md")["content"].as_str().unwrap();
        assert!(cmd.starts_with("---\ndescription: \"Z skill\"\n---") && cmd.contains("my-skills/zeta/SKILL.md"), "{cmd}");
        assert_eq!(spec, kit_spec(&[skills[1].clone(), skills[0].clone()]), "same skills, same kit");
        assert!(!kit_spec(&[]).contains("setup:"));
    }
}
