//! An `App` on a temp dir with a scripted `sbx`, for the daemon's tests.

use crate::app::{App, AppBuilder};
use crate::hostsys;
use crate::store::Paths;
use sbxw_core::config::Config;
use sbxw_sbx::runner::{Fake, Output};
use sbxw_sbx::Sbx;
use std::path::Path;
use std::sync::Arc;

/// What a well-behaved sbx answers when the test does not care.
pub fn sbx_answer(a: &[String]) -> Output {
    match a.first().map(String::as_str) {
        Some("version") => Output::ok("sbx version 0.48.0\n"),
        Some("ls") => Output::ok("SANDBOX   AGENT    STATUS\n"),
        Some("exec") if a.iter().any(|x| x == "node") => Output::ok("SBXW-TOOLING []\n"),
        _ => Output::ok(""),
    }
}

pub fn app_in(dir: &Path) -> Arc<App> {
    build(dir, Fake::new(sbx_answer))
}

fn build(dir: &Path, fake: Arc<Fake>) -> Arc<App> {
    AppBuilder {
        paths: Paths::under(dir),
        cfg: Config::default(),
        sbx: Sbx::new(fake),
        hosts: hostsys::for_tests(&dir.join("hosts")),
        use_api_key: true,
        sbx_version: Some("0.48.0".into()),
    }
    .build()
}

pub fn app() -> (tempfile::TempDir, Arc<App>) {
    let d = tempfile::tempdir().unwrap();
    let a = app_in(d.path());
    (d, a)
}

pub fn app_with(answer: impl Fn(&[String]) -> Output + Send + Sync + 'static) -> (tempfile::TempDir, Arc<App>, Arc<Fake>) {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new(answer);
    let a = build(d.path(), fake.clone());
    (d, a, fake)
}
