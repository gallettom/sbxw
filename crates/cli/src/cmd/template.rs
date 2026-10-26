//! `sbxw template`: snapshot a sandbox that is already set up, so new ones
//! start from it (`template = "<tag>"` in sbxw.toml) instead of re-running
//! every kit install. A local snapshot needs no registry request (sbx 0.48).
//!
//! A snapshot holds the sandbox's files outside the workspace, which includes
//! two secrets sbxw put there: the sandbox's agent token and its Claude OAuth
//! credentials. Both are removed before the save — a sandbox made from the
//! template gets its own on its first `sbxw up` — and put back afterwards.

use super::runtime;
use anyhow::{bail, Context, Result};
use sbxw_core::names::SandboxName;
use sbxw_core::templates::SECRETS;
use sbxw_sbx::Sbx;

pub fn save(name: &str, tag: &str) -> Result<()> {
    let name = SandboxName::parse(name)?;
    if tag.starts_with('-') || tag.trim().is_empty() {
        bail!("'{tag}' is not a template tag");
    }
    let rt = runtime();
    let sbx = Sbx::process();
    if !rt.block_on(sbx.exists(&name)) {
        bail!("no sandbox named '{name}' — `sbxw ls` lists them");
    }

    // Read, then remove, while the guest runs; a stopped one is started for it.
    if !rt.block_on(sbx.is_running(&name)) {
        println!("starting '{name}' to take its secrets out before the snapshot…");
        rt.block_on(sbx.run_detached(&name)).context("could not start the sandbox")?;
        if !rt.block_on(sbx.wait_until_running(&name, std::time::Duration::from_secs(60))) {
            bail!("'{name}' did not come up — nothing was saved");
        }
    }
    let mut kept: Vec<(&str, Vec<u8>)> = Vec::new();
    for path in SECRETS {
        let out = rt.block_on(sbx.exec(&name, &["cat", path], None))?;
        if out.success() {
            kept.push((path, out.stdout));
        }
    }
    let scrub: Vec<&str> = ["rm", "-f"].into_iter().chain(SECRETS).collect();
    let out = rt.block_on(sbx.exec(&name, &scrub, None))?;
    if !out.success() {
        bail!("could not remove the sandbox's secrets before the snapshot — nothing was saved: {}", String::from_utf8_lossy(&out.stderr).trim());
    }

    println!("saving '{name}' as template {tag}…");
    let saved = sbxw_sbx::interactive(&sbxw_sbx::template_save_args(&name, tag));

    // Put the secrets back whatever the save did: the sandbox is still in use.
    let mut restored = true;
    if rt.block_on(sbx.wait_until_running(&name, std::time::Duration::from_secs(5))) {
        for (path, data) in &kept {
            restored &= rt.block_on(sbx.write_file(&name, path, data, true)).is_ok();
        }
    } else {
        restored = kept.is_empty();
    }
    if !restored {
        eprintln!("note: '{name}' is stopped — its agent token and credentials come back on `sbxw up {name}`");
    }
    saved?;
    println!("saved {tag}. Start new sandboxes from it with, in sbxw.toml:\n\n  template = \"{tag}\"\n");
    println!("It holds this sandbox's installed tools and files outside the workspace (secrets removed) — keep it on this host.");
    Ok(())
}

pub fn ls() -> Result<()> {
    sbxw_sbx::interactive(&["template".into(), "ls".into()])
}
