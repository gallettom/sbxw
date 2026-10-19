//! A bring-up's progress, in the terminal: the same steps the browser's
//! corner card shows, ticked off as they complete, with `sbx`'s own output
//! (the image pull, mostly) on one line under the step that is running.

use sbxw_core::progress::{Progress, Step};
use std::io::{IsTerminal, Write};

pub struct Renderer {
    steps: Vec<Step>,
    current: Option<String>,
    tty: bool,
    detail_shown: bool,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            steps: vec![],
            current: None,
            tty: std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
            detail_shown: false,
        }
    }

    fn clear_detail(&mut self) {
        if self.tty && self.detail_shown {
            eprint!("\r\x1b[2K");
            self.detail_shown = false;
        }
    }

    fn label(&self, id: &str) -> String {
        self.steps.iter().find(|s| s.id == id).map(|s| s.label.clone()).unwrap_or_else(|| id.to_string())
    }

    fn finish_current(&mut self) {
        if let Some(id) = self.current.take() {
            self.clear_detail();
            eprintln!("  ✓ {}", self.label(&id));
        }
    }

    pub fn show(&mut self, p: &Progress) {
        match p {
            Progress::Plan { steps } => {
                self.steps = steps.clone();
                let n = steps.len();
                eprintln!("sbxw  {n} step{}", if n == 1 { "" } else { "s" });
            }
            Progress::Started { id } => {
                self.finish_current();
                self.current = Some(id.clone());
                if !self.tty {
                    eprintln!("  ● {}", self.label(id));
                }
            }
            Progress::Detail { text } => {
                if self.tty {
                    let label = self.current.as_deref().map(|c| self.label(c)).unwrap_or_default();
                    let width = 100usize;
                    let line: String = format!("  ● {label} — {text}").chars().take(width).collect();
                    eprint!("\r\x1b[2K\x1b[2m{line}\x1b[0m");
                    let _ = std::io::stderr().flush();
                    self.detail_shown = true;
                } else {
                    eprintln!("      {text}");
                }
            }
            Progress::Warning { text } => {
                self.clear_detail();
                eprintln!("  ! {}", text.replace('\n', "\n    "));
            }
            Progress::Done { error } => {
                if error.is_none() {
                    self.finish_current();
                } else {
                    self.clear_detail();
                    if let Some(id) = self.current.take() {
                        eprintln!("  ✕ {}", self.label(&id));
                    }
                }
            }
        }
    }
}
