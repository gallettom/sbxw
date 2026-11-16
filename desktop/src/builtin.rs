//! What the app has built in, written out for the daemon to show Jarvis.
//!
//! At every start the app writes `<sbxw home>/desktop/builtin/`
//! (`~/.sbxw/desktop/builtin/`): `screens/<name>.luau` (each built-in
//! screen as compiled in), `screens.md` (the screen contract,
//! `docs/screens.md`) and `intents.json` (the closed list of intents with
//! their arguments and risk). The folder is replaced whole: written next to
//! it under a temporary name, then renamed into place, so a reader sees the
//! old set or the new one, never half of each. The app never reads anything
//! back from it: overrides live in `<sbxw home>/desktop/screens/`.
//!
//! `intents.json`:
//!
//! ```json
//! {"intents": [{"name": "stop_sandbox", "args": {"sandbox": "string, required"},
//!               "risk": "direct", "doc": "..."},
//!              {"name": "approve_proposal", "args": {"id": "string, required"},
//!               "risk": "approval", "asks": "Approve a Jarvis proposal", "doc": "..."}]}
//! ```

use crate::intent::{Arg, Risk};
use serde_json::{json, Map, Value as J};
use std::path::{Path, PathBuf};

pub const SCREENS_MD: &str = include_str!("../docs/screens.md");

/// `$HOME/.sbxw/desktop/builtin`, where the daemon (`Paths::from_home`) looks.
pub fn default_dir() -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join(".sbxw").join("desktop").join("builtin")
}

fn arg_type(a: Arg) -> String {
    match a {
        Arg::Str => "string".into(),
        Arg::Text => "string, may be empty".into(),
        Arg::OneOf(set) => format!("one of {}", set.join(", ")),
        Arg::Int(lo, hi) if hi == i64::MAX => format!("whole number from {lo}"),
        Arg::Int(lo, hi) => format!("whole number {lo} to {hi}"),
        Arg::Pane => "\"focused\", \"spare\" or a pane number 1 to 4".into(),
        Arg::Bool => "true or false".into(),
        Arg::Url => "an http:// or https:// URL".into(),
    }
}

/// The registry as JSON.
pub fn intents_json() -> J {
    let list: Vec<J> = crate::intent::all()
        .map(|s| {
            let mut args = Map::new();
            for (name, ty, required) in s.args {
                args.insert(name.to_string(), J::String(format!("{}{}", arg_type(*ty), if *required { ", required" } else { "" })));
            }
            let mut v = json!({ "name": s.name, "args": args, "doc": s.doc });
            match s.risk {
                Risk::Direct => v["risk"] = "direct".into(),
                Risk::Approval(asks) => {
                    v["risk"] = "approval".into();
                    v["asks"] = asks.into();
                }
            }
            v
        })
        .collect();
    json!({ "intents": list })
}

/// Writes the set into `dir`, replacing what was there.
pub fn export(dir: &Path) -> std::io::Result<()> {
    let parent = dir.parent().ok_or_else(|| std::io::Error::other("no parent folder"))?;
    std::fs::create_dir_all(parent)?;
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "builtin".into());
    let tmp = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("screens"))?;
    for (screen, src) in crate::screen::all_builtin() {
        std::fs::write(tmp.join("screens").join(format!("{screen}.luau")), src)?;
    }
    std::fs::write(tmp.join("screens.md"), SCREENS_MD)?;
    std::fs::write(tmp.join("intents.json"), serde_json::to_string_pretty(&intents_json()).expect("json") + "\n")?;
    // Swap: the old set moves aside, the new one takes its name.
    let old = parent.join(format!(".{name}.old-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&old);
    if dir.exists() {
        std::fs::rename(dir, &old)?;
    }
    if let Err(e) = std::fs::rename(&tmp, dir) {
        let _ = std::fs::rename(&old, dir);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_every_builtin_screen_the_doc_and_the_intents_replacing_the_old_set() {
        let root = std::env::temp_dir().join(format!("sbxw-builtin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("desktop").join("builtin");
        std::fs::create_dir_all(dir.join("screens")).unwrap();
        std::fs::write(dir.join("screens").join("gone.luau"), "old").unwrap();
        export(&dir).unwrap();
        assert!(!dir.join("screens/gone.luau").exists(), "replaced whole");
        for (name, src) in crate::screen::all_builtin() {
            assert!(name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{name}: the daemon takes [a-z0-9-] names");
            assert_eq!(std::fs::read_to_string(dir.join(format!("screens/{name}.luau"))).unwrap(), src);
        }
        let md = std::fs::read_to_string(dir.join("screens.md")).unwrap();
        assert!(md.contains("### `markdown`"));
        let v: J = serde_json::from_str(&std::fs::read_to_string(dir.join("intents.json")).unwrap()).unwrap();
        let list = v["intents"].as_array().unwrap();
        assert_eq!(list.len(), crate::intent::all().count());
        let ap = list.iter().find(|i| i["name"] == "approve_proposal").unwrap();
        assert_eq!((ap["risk"].as_str(), ap["args"]["id"].as_str()), (Some("approval"), Some("string, required")));
        assert_eq!(list.iter().find(|i| i["name"] == "stop_sandbox").unwrap()["risk"], "direct");
        // No temporary folder left behind.
        let left: Vec<_> = std::fs::read_dir(dir.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, vec!["builtin".to_string()]);
        export(&dir).unwrap();
        std::fs::remove_dir_all(&root).ok();
    }
}
