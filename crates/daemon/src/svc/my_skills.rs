//! The Skills dialog's "keep this skill": copy a skill out of a sandbox into
//! `~/.sbxw/skills/`, and the kit that puts every kept skill in the next
//! sandboxes ([`sbxw_core::my_skills`]).
//!
//! Only the UI listener reaches this: a sandbox can offer a skill by writing
//! it, but only the person keeps it, since it then reaches every sandbox
//! created afterwards. The copy is data — files at `0600`, never executable,
//! never under the host's own `~/.claude/skills`.

use crate::app::App;
use crate::store::write_atomic;
use anyhow::{anyhow, bail, Context, Result};
use sbxw_core::my_skills::{self, File, Manifest, Origin, Skill};
use sbxw_core::SandboxName;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Export {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    path: String,
    #[serde(default)]
    files: Vec<File>,
    #[serde(default)]
    skipped: Vec<String>,
}

/// Every file under `dir`, relative to it, the manifest left out.
fn read_files(dir: &Path, executable: &[String]) -> Result<Vec<File>> {
    fn walk(root: &Path, dir: &Path, executable: &[String], out: &mut Vec<File>) -> Result<()> {
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let p = e.path();
            let ty = e.file_type()?;
            if ty.is_dir() {
                walk(root, &p, executable, out)?;
            } else if ty.is_file() {
                let rel = p.strip_prefix(root)?.to_string_lossy().into_owned();
                if rel == my_skills::MANIFEST {
                    continue;
                }
                let content = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
                out.push(File { executable: executable.contains(&rel), path: rel, content });
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, executable, &mut out)?;
    Ok(out)
}

fn read_manifest(dir: &Path) -> Manifest {
    std::fs::read(dir.join(my_skills::MANIFEST))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

impl App {
    /// The kept skills, name order, as `(name, folder)`. A folder whose name
    /// sbxw would not have written is not one of them.
    fn kept_dirs(&self) -> Vec<(String, PathBuf)> {
        let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(self.store.paths.my_skills())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| {
                let name = e.file_name().to_str()?.to_string();
                my_skills::valid_name(&name).then(|| (name, e.path()))
            })
            .collect();
        out.sort();
        out
    }

    /// The kit for the kept skills, written out; `None` when there are none,
    /// so a person who never kept one gets no extra kit.
    pub fn my_skills_kit(&self) -> Result<Option<PathBuf>> {
        let skills = self
            .kept_dirs()
            .into_iter()
            .map(|(name, dir)| {
                let files = read_files(&dir, &read_manifest(&dir).executable)?;
                Ok(Skill { name, files })
            })
            .collect::<Result<Vec<_>>>()?;
        let dir = self.store.paths.my_skills_kit();
        if skills.is_empty() {
            // Kept for a sandbox that holds the path, emptied: sbx re-reads it.
            if dir.exists() {
                write_atomic(&dir.join("spec.yaml"), my_skills::kit_spec(&[]).as_bytes())?;
            }
            return Ok(None);
        }
        let spec = my_skills::kit_spec(&skills);
        let file = dir.join("spec.yaml");
        if std::fs::read_to_string(&file).ok().as_deref() != Some(spec.as_str()) {
            write_atomic(&file, spec.as_bytes())?;
        }
        Ok(Some(dir))
    }

    /// The kept skills, for the dialog.
    pub fn my_skills_list(&self) -> Value {
        let skills: Vec<Value> = self
            .kept_dirs()
            .into_iter()
            .map(|(name, dir)| {
                let m = read_manifest(&dir);
                let description = std::fs::read_to_string(dir.join("SKILL.md")).ok().and_then(|s| my_skills::description(&s));
                json!({ "name": name, "description": description, "sandbox": m.sandbox, "path": m.path, "saved": m.saved })
            })
            .collect();
        json!({ "skills": skills, "dir": self.store.paths.my_skills() })
    }

    /// Run [`my_skills::SANDBOX_SCRIPT`] in `sb`: `mode`, the workspace, then `rest`.
    async fn skill_script(&self, sb: &SandboxName, mode: &str, rest: &[&str]) -> Result<String> {
        let ws = self.store.workspace(sb).map(|w| w.to_string_lossy().into_owned()).unwrap_or_default();
        // `sbx exec` refuses an empty element: values go in behind `=`.
        let values: Vec<String> = [mode, ws.as_str()].iter().chain(rest).map(|a| format!("={a}")).collect();
        let mut argv = vec!["node", "-"];
        argv.extend(values.iter().map(String::as_str));
        let out = self.sbx.exec(sb, &argv, Some(my_skills::SANDBOX_SCRIPT.as_bytes().to_vec())).await?;
        if !out.success() {
            bail!("reading skills in '{sb}' failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(out.stdout_str())
    }

    /// The skills a sandbox has that could be kept: its project's
    /// `.claude/skills/*` and its own `~/.claude/skills/*` (unless that is
    /// sbx's store), each marked when one of that name is kept already.
    pub async fn sandbox_skills(&self, sb: &SandboxName) -> Result<Value> {
        let raw = self.skill_script(sb, "list", &[]).await?;
        let mut v: Value = serde_json::from_str(raw.trim()).context("reading the sandbox's skill list")?;
        let kept: Vec<String> = self.kept_dirs().into_iter().map(|(n, _)| n).collect();
        if let Some(list) = v["skills"].as_array_mut() {
            for s in list {
                let name = s["name"].as_str().unwrap_or_default().to_string();
                s["kept"] = json!(kept.contains(&name));
                s["keepable"] = json!(my_skills::valid_name(&name));
            }
        }
        Ok(json!({ "skills": v["skills"] }))
    }

    /// Copy one skill out of `sb` and keep it. Replacing a kept skill of the
    /// same name takes `replace`: it changes what every next sandbox gets.
    pub async fn keep_skill(&self, sb: &SandboxName, origin: Origin, name: &str, replace: bool) -> Result<Vec<String>> {
        if !my_skills::valid_name(name) {
            bail!("'{name}' is not a skill name sbxw keeps: lowercase letters, digits, - and _");
        }
        let target = self.store.paths.my_skills().join(name);
        if target.exists() && !replace {
            bail!("a skill named '{name}' is kept already — replace it, or rename this one first");
        }
        let raw = self.skill_script(sb, "export", &[origin.as_str(), name]).await?;
        let ex: Export = serde_json::from_str(raw.trim()).context("reading the skill out of the sandbox")?;
        if !ex.ok {
            bail!("{}", ex.error.unwrap_or_else(|| "the sandbox did not hand the skill over".into()));
        }
        my_skills::check(name, &ex.files).map_err(|e| anyhow!(e))?;

        // Staged beside the target, then swapped in: a failure leaves the
        // previous copy, never half of each.
        let root = self.store.paths.my_skills();
        let staging = root.join(format!(".{name}.{}.new", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        let written = (|| -> Result<()> {
            for f in &ex.files {
                write_atomic(&staging.join(&f.path), f.content.as_bytes())?;
            }
            let manifest = Manifest {
                sandbox: sb.to_string(),
                path: ex.path.clone(),
                saved: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
                executable: ex.files.iter().filter(|f| f.executable).map(|f| f.path.clone()).collect(),
            };
            write_atomic(&staging.join(my_skills::MANIFEST), &serde_json::to_vec_pretty(&manifest)?)?;
            Ok(())
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
        let old = root.join(format!(".{name}.{}.old", std::process::id()));
        if target.exists() {
            std::fs::rename(&target, &old)?;
        }
        std::fs::rename(&staging, &target)?;
        let _ = std::fs::remove_dir_all(&old);
        self.my_skills_kit()?;

        let mut warnings: Vec<String> = Vec::new();
        if !ex.skipped.is_empty() {
            warnings.push(format!("left out: {}", ex.skipped.join(", ")));
        }
        Ok(warnings)
    }

    /// Stop keeping a skill: sandboxes created from now on do not get it.
    pub fn forget_skill(&self, name: &str) -> Result<()> {
        if !my_skills::valid_name(name) {
            bail!("'{name}' is not a kept skill");
        }
        let dir = self.store.paths.my_skills().join(name);
        if !dir.exists() {
            bail!("no kept skill named '{name}'");
        }
        std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
        self.my_skills_kit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{app_with, sbx_answer};
    use sbxw_core::my_skills::Origin;
    use sbxw_core::SandboxName;
    use sbxw_sbx::runner::Output;

    fn export(a: &[String]) -> Output {
        if a.iter().any(|x| x == "=export") {
            let name = a.last().unwrap().trim_start_matches('=');
            let files = if name == "evil" {
                serde_json::json!([{ "path": "SKILL.md", "content": "x" }, { "path": "../../.zshrc", "content": "x" }])
            } else {
                serde_json::json!([
                    { "path": "SKILL.md", "content": "---\ndescription: Mine\n---\n" },
                    { "path": "bin/go.sh", "content": "#!/bin/sh\n", "executable": true },
                ])
            };
            return Output::ok(&serde_json::json!({ "ok": true, "path": format!("/w/.claude/skills/{name}"), "files": files, "skipped": [] }).to_string());
        }
        sbx_answer(a)
    }

    #[tokio::test]
    async fn a_kept_skill_reaches_the_kit_and_leaves_it_when_forgotten() {
        let (_d, app, fake) = app_with(export);
        let sb = SandboxName::parse("neos").unwrap();
        assert_eq!(app.my_skills_kit().unwrap(), None, "no kept skill, no kit");

        app.keep_skill(&sb, Origin::Workspace, "mine", false).await.unwrap();
        let call = fake.calls.lock().unwrap().clone().into_iter().find(|c| c.iter().any(|x| x == "=export")).unwrap();
        assert!(call.ends_with(&["=export".into(), "=".into(), "=workspace".into(), "=mine".into()]), "{call:?}");
        let kept = app.store.paths.my_skills().join("mine");
        assert!(kept.join("bin/go.sh").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(kept.join("bin/go.sh")).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0, "never executable on the host");
        }
        let spec = std::fs::read_to_string(app.my_skills_kit().unwrap().unwrap().join("spec.yaml")).unwrap();
        assert!(spec.contains("my-skills/mine/bin/go.sh\"\n      mode: \"0755\""), "{spec}");
        assert_eq!(app.my_skills_list()["skills"][0]["description"], "Mine");

        let again = app.keep_skill(&sb, Origin::Workspace, "mine", false).await.unwrap_err();
        assert!(format!("{again:#}").contains("kept already"));
        app.keep_skill(&sb, Origin::Workspace, "mine", true).await.unwrap();

        app.forget_skill("mine").unwrap();
        assert!(!kept.exists());
        assert_eq!(app.my_skills_kit().unwrap(), None);
    }

    #[tokio::test]
    async fn a_path_out_of_the_skill_is_refused_and_nothing_is_written() {
        let (_d, app, _) = app_with(export);
        let sb = SandboxName::parse("neos").unwrap();
        assert!(app.keep_skill(&sb, Origin::Home, "evil", false).await.is_err());
        assert!(app.keep_skill(&sb, Origin::Home, "../x", false).await.is_err());
        let left: Vec<_> = std::fs::read_dir(app.store.paths.my_skills()).into_iter().flatten().collect();
        assert!(left.is_empty(), "{left:?}");
    }
}
