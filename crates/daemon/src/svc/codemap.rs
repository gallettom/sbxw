//! Background code map runs: refused up front when they cannot succeed,
//! started as a headless session, ended by the agent's report or its exit.

use crate::app::{now_ms, App};
use crate::bus::Event;
use sbxw_core::codemap::{self, Kind, Run};
use sbxw_core::tooling::{CODEMAP_COMMAND_PATH, CODEMAP_LENS_COMMAND_PATH};
use sbxw_core::SandboxName;
use std::sync::Arc;
use std::time::Duration;

/// Generous: a large repository's map legitimately takes tens of minutes. It
/// bounds a session that stopped making progress, nothing else.
const CODEMAP_MAX: Duration = Duration::from_secs(45 * 60);
const NOTE_LINES: usize = 3;

impl App {
    pub fn codemap_runs(&self) -> Vec<Run> {
        self.codemap.lock().unwrap().all()
    }

    pub fn codemap_run(&self, sandbox: &str) -> Option<Run> {
        self.codemap.lock().unwrap().get(sandbox).cloned()
    }

    /// Write the map (`lens: None`) or retell it for a reader. The three
    /// refusals a run would otherwise discover minutes later come first: a
    /// stopped sandbox, no command (the kit is not applied), already writing.
    pub async fn codemap_start(self: &Arc<Self>, sandbox: &SandboxName, lens: Option<&str>) -> Result<(Run, &'static str), String> {
        let (kind, prompt, command, path, dir) = match lens {
            None => (Kind::Map, "/codemap".to_string(), "/codemap", CODEMAP_COMMAND_PATH, codemap::CODEMAP_DIR),
            Some(brief) => {
                let brief = codemap::lens_objective(brief)?;
                (Kind::Lens, format!("/codemap-lens {brief}"), "/codemap-lens", CODEMAP_LENS_COMMAND_PATH, codemap::LENS_DIR)
            }
        };
        if let Some(k) = self.codemap.lock().unwrap().writing(sandbox) {
            return Err(codemap::already_writing(sandbox, k));
        }
        if !self.sbx.is_running(sandbox).await {
            return Err(format!("'{sandbox}' is not running — start it first, then ask again"));
        }
        if !self.sbx.file_exists(sandbox, path).await {
            return Err(format!(
                "'{sandbox}' has no {command} command — add the codemap kit to this project \
                 (`kits = [\"…/assets/codemap\"]` in sbxw.toml) and run `sbxw up`"
            ));
        }
        // Claimed before the session starts, so two clicks cannot both start one.
        let run = self.codemap.lock().unwrap().begin(sandbox, kind, now_ms())?;
        self.send(Event::Codemap(run.clone()));
        tracing::info!("codemap: '{sandbox}' is writing {} under {dir}/", kind.what());
        let app = self.clone();
        let sb = sandbox.clone();
        tokio::spawn(async move { app.run_codemap_session(&sb, kind, &prompt).await });
        Ok((run, dir))
    }

    async fn run_codemap_session(&self, sandbox: &SandboxName, kind: Kind, prompt: &str) {
        let args = sbxw_sbx::scripts::codemap_run_args(sandbox, self.cfg.agent_port(), prompt);
        let out = tokio::time::timeout(CODEMAP_MAX, self.sbx.raw(sbxw_sbx::runner::Invocation::new(args))).await;
        let (ok, note) = match out {
            Err(_) => (false, format!("still going after {} minutes — stopped", CODEMAP_MAX.as_secs() / 60)),
            Ok(Err(e)) => (false, format!("could not start a session: {e:#}")),
            Ok(Ok(o)) if o.success() => (true, codemap::output_tail(&o.stdout, NOTE_LINES)),
            Ok(Ok(o)) => {
                let tail = match codemap::output_tail(&o.stderr, NOTE_LINES) {
                    t if t.is_empty() => codemap::output_tail(&o.stdout, NOTE_LINES),
                    t => t,
                };
                (false, format!("exit {} — {tail}", o.code.unwrap_or(-1)))
            }
        };
        if self.codemap_finish(sandbox, ok, Some(&note)).is_some() {
            if ok {
                tracing::info!("codemap: '{sandbox}' finished {} — {note}", kind.what());
            } else {
                tracing::warn!("codemap: '{sandbox}' failed to write {} — {note}", kind.what());
            }
        }
    }

    /// The agent (or the exit) says the run is over; the second report is ignored.
    pub fn codemap_finish(&self, sandbox: &str, ok: bool, note: Option<&str>) -> Option<Run> {
        let run = self.codemap.lock().unwrap().finish(sandbox, ok, note, now_ms())?;
        self.send(Event::Codemap(run.clone()));
        Some(run)
    }
}
