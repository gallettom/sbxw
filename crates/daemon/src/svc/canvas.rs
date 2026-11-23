//! Canvases on the host's disk (the person's data): one directory per
//! canvas under `state/canvas/<id>/`, its manifest `canvas.json` and its
//! pictures, each under a name generated here. The rules are in
//! `sbxw_core::canvas`; this file keeps them on disk and serves them.
//!
//! * A path is only ever built from a checked id ([`canvas::valid_id`]) and a
//!   checked, generated file name the manifest lists ([`Canvas::file_format`]).
//!   Nothing an agent or a person sends is joined to a directory.
//! * Writes are atomic (a private temporary file and a rename), the folders
//!   0700, the files 0600.
//! * The log gets ids, counts and sizes, never a title, a remark or a picture.
//! * An agent reaches only its own sandbox's canvases (the token's sandbox,
//!   never a body field), can add shots and diagrams, replace its own
//!   diagram, and read remarks and the person's diagram edits, nothing else.
//! * A diagram lives in the manifest ([`canvas::DiagramItem`]); the core
//!   parses, imports, cleans, checks and lays it out.

use crate::app::{now_ms, App};
use crate::bus::Event;
use sbxw_core::canvas::{self, Canvas, DiagramRead, Edit, SaveError, Summary};
use sbxw_core::diagram;
use sbxw_core::SandboxRef;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// The canvases, read from disk once.
#[derive(Default)]
pub struct Book {
    loaded: bool,
    by_id: HashMap<String, Canvas>,
}

/// What the `canvas` event says: which canvas changed, of which sandbox.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct CanvasNudge {
    pub id: String,
    pub sandbox: String,
    /// What changed: `shots`, `diagram`, `edit`, `review`, `read`, `deleted`, `created`, `renamed`, `asset`.
    pub what: &'static str,
}

/// A shot as an agent posts it.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct ShotIn {
    pub b64: String,
    pub title: String,
    pub note: String,
    pub width: Option<u32>,
    pub screen: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Added {
    pub canvas: String,
    pub title: String,
    pub round: u32,
    pub shot: String,
}

/// A diagram as an agent posts it: the file's text, untouched.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct DiagramIn {
    pub json: String,
    /// The diagram's title, over the file's.
    pub title: String,
    /// `workflow`, `sequence`, `dataflow` or `lifecycle`: fills a missing kind, refused when it differs.
    pub kind: Option<diagram::Kind>,
    /// One of the agent's own diagrams on that canvas, to send a new version of.
    pub replace: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct DiagramAdded {
    pub canvas: String,
    pub canvas_title: String,
    pub diagram: String,
    pub title: String,
    pub kind: &'static str,
    pub rev: u32,
    pub round: u32,
    pub nodes: usize,
    pub edges: usize,
    pub replaced: bool,
}

/// What `canvas_read` gives the agent: the remarks as text, and the diagrams
/// the person changed, each with the canvas it is on.
#[derive(Debug, Serialize, PartialEq, Default)]
pub struct AgentRead {
    pub text: String,
    pub diagrams: Vec<(String, DiagramRead)>,
}

/// Why a person's diagram save failed.
#[derive(Debug, PartialEq)]
pub enum DiagramSaveError {
    /// Another save got there first: the diagram's revision now.
    Stale(u32),
    NotFound,
    Bad(String),
}

impl App {
    fn canvas_root(&self) -> PathBuf {
        self.store.paths.state().join("canvas")
    }

    /// The directory of a checked id.
    fn canvas_dir(&self, id: &str) -> Option<PathBuf> {
        canvas::valid_id(id).then(|| self.canvas_root().join(id))
    }

    fn canvas_book<R>(&self, f: impl FnOnce(&mut Book) -> R) -> R {
        let mut g = self.canvas.lock().unwrap();
        if !g.loaded {
            g.loaded = true;
            for e in std::fs::read_dir(self.canvas_root()).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !canvas::valid_id(&name) {
                    continue;
                }
                match std::fs::read_to_string(e.path().join("canvas.json")).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<Canvas>(&t).map_err(|e| e.to_string())) {
                    Ok(mut c) if c.id == name => {
                        // Saved before grids: rounds nobody arranged become grids now.
                        if c.relayout_legacy() {
                            match self.canvas_save(&c) {
                                Ok(()) => tracing::info!("canvas {name}: laid out again as a grid ({} shots)", c.shots.len()),
                                Err(e) => tracing::warn!("canvas {name}: laid out again as a grid, not saved: {e}"),
                            }
                        }
                        g.by_id.insert(name, c);
                    }
                    Ok(_) => tracing::warn!("canvas {name}: its manifest names another id; skipped"),
                    Err(e) => tracing::warn!("canvas {name}: manifest unreadable, skipped: {e}"),
                }
            }
        }
        f(&mut g)
    }

    fn canvas_save(&self, c: &Canvas) -> Result<(), String> {
        let dir = self.canvas_dir(&c.id).ok_or("bad canvas id")?;
        let body = serde_json::to_vec_pretty(c).map_err(|e| e.to_string())?;
        crate::store::write_atomic(&dir.join("canvas.json"), &body).map_err(|e| format!("could not save the canvas: {e:#}"))
    }

    fn canvas_changed(&self, c: &Canvas, what: &'static str) {
        self.send(Event::Canvas(CanvasNudge { id: c.id.clone(), sandbox: c.sandbox.clone(), what }));
    }

    /// Every canvas, newest first, with the total size of their pictures.
    pub fn canvas_list(&self) -> (Vec<Summary>, u64) {
        self.canvas_book(|b| {
            let mut v: Vec<Summary> = b.by_id.values().map(Canvas::summary).collect();
            v.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms).then(a.id.cmp(&b.id)));
            let total = v.iter().map(|s| s.bytes).sum();
            (v, total)
        })
    }

    /// Per sandbox, how many canvases hold shots added since the last round
    /// of remarks was sent (a canvas never reviewed: any shot). Counts only.
    pub(crate) fn canvas_new_counts(&self) -> std::collections::HashMap<String, usize> {
        self.canvas_book(|b| {
            let mut out = std::collections::HashMap::new();
            for c in b.by_id.values() {
                let since = c.reviews.last().map_or(0, |r| r.at_ms);
                if c.shots.iter().any(|s| s.added_ms > since) {
                    *out.entry(c.sandbox.clone()).or_insert(0) += 1;
                }
            }
            out
        })
    }

    pub fn canvas_get(&self, id: &str) -> Option<Canvas> {
        self.canvas_book(|b| b.by_id.get(id).cloned())
    }

    /// A picture's bytes and type, by a file name the manifest lists.
    pub fn canvas_image(&self, id: &str, file: &str) -> Result<(Vec<u8>, &'static str), String> {
        let fmt = self.canvas_book(|b| b.by_id.get(id).and_then(|c| c.file_format(file))).ok_or("no such picture")?;
        let dir = self.canvas_dir(id).ok_or("no such picture")?;
        let bytes = std::fs::read(dir.join(file)).map_err(|_| "the picture's file is missing".to_string())?;
        Ok((bytes, fmt.mime()))
    }

    pub fn canvas_create(&self, sandbox: &SandboxRef, title: &str) -> Result<Canvas, String> {
        let c = Canvas::new(canvas::new_id(), sandbox.to_string(), title, now_ms());
        self.canvas_save(&c)?;
        self.canvas_book(|b| b.by_id.insert(c.id.clone(), c.clone()));
        tracing::info!("canvas {}: created for {}", c.id, c.sandbox);
        self.canvas_changed(&c, "created");
        Ok(c)
    }

    /// Changes canvas `id` with `f` and saves it; `Err` leaves it as it was.
    fn canvas_update<R>(&self, id: &str, what: &'static str, f: impl FnOnce(&mut Canvas) -> Result<R, String>) -> Result<(R, Canvas), String> {
        let (r, c) = self.canvas_book(|b| {
            let c = b.by_id.get_mut(id).ok_or("no such canvas")?;
            let mut next = c.clone();
            let r = f(&mut next)?;
            self.canvas_save(&next)?;
            *c = next.clone();
            Ok::<_, String>((r, next))
        })?;
        self.canvas_changed(&c, what);
        Ok((r, c))
    }

    pub fn canvas_rename(&self, id: &str, title: &str) -> Result<Canvas, String> {
        let t = canvas::clean(title, canvas::MAX_TITLE, false);
        if t.is_empty() {
            return Err("a canvas needs a title".into());
        }
        self.canvas_update(id, "renamed", |c| {
            c.title = t;
            c.updated_ms = now_ms();
            Ok(())
        })
        .map(|(_, c)| c)
    }

    /// Stores a picture's bytes under a generated name in canvas `id`'s
    /// directory, after checking its header and the canvas's room.
    fn canvas_store_picture(&self, id: &str, kind: char, bytes: &[u8]) -> Result<(String, canvas::Header), String> {
        let h = canvas::sniff(bytes)?;
        let used = self.canvas_book(|b| b.by_id.get(id).map(Canvas::bytes)).ok_or("no such canvas")?;
        if used + bytes.len() as u64 > canvas::MAX_CANVAS_BYTES {
            return Err(format!("the canvas would hold more than {} MB of pictures; delete older canvases or start a new one", canvas::MAX_CANVAS_BYTES >> 20));
        }
        let file = canvas::new_file(kind, h.format);
        let dir = self.canvas_dir(id).ok_or("bad canvas id")?;
        crate::store::write_atomic(&dir.join(&file), bytes).map_err(|e| format!("could not store the picture: {e:#}"))?;
        Ok((file, h))
    }

    fn canvas_drop_files(&self, id: &str, files: &[String]) {
        let Some(dir) = self.canvas_dir(id) else { return };
        for f in files.iter().filter(|f| canvas::valid_file(f)) {
            let _ = std::fs::remove_file(dir.join(f));
        }
    }

    /// The agent of `me` adds one shot. `canvas`: one of its own canvases,
    /// else its most recently changed one, else a new one titled `title`.
    /// `batch`: the tool call's, so its shots form one round.
    pub fn canvas_agent_add(&self, me: &SandboxRef, canvas_id: Option<&str>, title: &str, batch: &str, shot: ShotIn) -> Result<Added, String> {
        let id = self.canvas_agent_target(me, canvas_id, title, "Review")?;
        let bytes = canvas::unbase64(&shot.b64, canvas::MAX_IMAGE_BYTES)?;
        // Room in the round before the picture is written.
        let full = self.canvas_book(|b| {
            b.by_id.get(&id).is_some_and(|c| c.rounds.last().is_some_and(|r| !batch.is_empty() && r.batch == batch && r.shots as usize >= canvas::MAX_SHOTS_PER_ROUND))
        });
        if full {
            return Err(format!("a round holds at most {} shots", canvas::MAX_SHOTS_PER_ROUND));
        }
        let (file, h) = self.canvas_store_picture(&id, 's', &bytes)?;
        let size = bytes.len() as u64;
        let res = self.canvas_update(&id, "shots", |c| {
            let now = now_ms();
            let round = c.round_for(batch, now)?;
            let s = c.add_shot(round, file.clone(), h, size, &shot.title, &shot.note, shot.width, shot.screen.as_deref(), now);
            Ok((round, s.id.clone()))
        });
        match res {
            Ok(((round, sid), c)) => {
                tracing::info!("canvas {}: a shot added by {} (round {round}, {} KB, {}x{})", c.id, c.sandbox, size / 1024, h.width, h.height);
                Ok(Added { canvas: c.id, title: c.title, round, shot: sid })
            }
            Err(e) => {
                self.canvas_drop_files(&id, &[file]);
                Err(e)
            }
        }
    }

    /// The canvas an agent's post goes to: `canvas_id` when it is one of its
    /// own sandbox's, else its most recently changed one, else (or with a
    /// `title`) a new one titled `title` (or `fallback`).
    fn canvas_agent_target(&self, me: &SandboxRef, canvas_id: Option<&str>, title: &str, fallback: &str) -> Result<String, String> {
        let mine = me.to_string();
        Ok(match canvas_id.filter(|s| !s.is_empty()) {
            Some(id) => {
                let ok = self.canvas_book(|b| b.by_id.get(id).is_some_and(|c| c.sandbox == mine));
                if !ok {
                    return Err("no canvas of this sandbox has that id (canvas_list shows them)".into());
                }
                id.to_string()
            }
            None => match self.canvas_current(me) {
                Some(id) if title.trim().is_empty() => id,
                _ => self.canvas_create(me, if title.trim().is_empty() { fallback } else { title })?.id,
            },
        })
    }

    /// The agent of `me` adds a diagram to one of its canvases (or its
    /// current one, or a new one), or sends a new version of one of its
    /// diagrams (`replace`, on that diagram's canvas). The file's JSON is
    /// sbxw's schema or Archify's, checked and laid out by the core.
    pub fn canvas_agent_diagram(&self, me: &SandboxRef, canvas_id: Option<&str>, batch: &str, input: DiagramIn) -> Result<DiagramAdded, String> {
        let mine = me.to_string();
        // Checked before any canvas is made for it.
        let d = canvas::ingest_diagram(&input.json, input.kind)?;
        let replace = input.replace.filter(|r| !r.is_empty());
        if let Some(r) = &replace {
            if !diagram::valid_diagram_id(r) {
                return Err("replace: not a diagram id (d- and 12 hex digits)".into());
            }
        }
        let id = match (&replace, canvas_id.filter(|s| !s.is_empty())) {
            // The diagram says which canvas, among this sandbox's.
            (Some(r), None) => self
                .canvas_book(|b| b.by_id.values().find(|c| c.sandbox == mine && c.diagrams.iter().any(|x| &x.id == r)).map(|c| c.id.clone()))
                .ok_or("no diagram of this sandbox has that id")?,
            (_, given) => {
                let fallback = if d.title.is_empty() { "Diagrams" } else { d.title.as_str() };
                self.canvas_agent_target(me, given, "", fallback)?
            }
        };
        let in_bytes = input.json.len();
        let ((round, item), c) = self.canvas_update(&id, "diagram", |c| {
            let now = now_ms();
            match &replace {
                Some(r) => {
                    let it = c.replace_diagram(r, d, &input.title, now)?;
                    Ok((it.round, it.clone()))
                }
                None => {
                    let round = c.round_for(batch, now)?;
                    let it = c.add_diagram(round, d, &input.title, now)?;
                    Ok((round, it.clone()))
                }
            }
        })?;
        tracing::info!(
            "canvas {}: diagram {} {} by {} (rev {}, {} nodes, {} edges, {} KB sent, {} KB stored)",
            c.id,
            item.id,
            if replace.is_some() { "replaced" } else { "added" },
            c.sandbox,
            item.diagram.rev,
            item.diagram.nodes.len(),
            item.diagram.edges.len(),
            in_bytes / 1024,
            item.bytes / 1024
        );
        Ok(DiagramAdded {
            canvas: c.id,
            canvas_title: c.title,
            diagram: item.id,
            title: item.title,
            kind: item.kind.name(),
            rev: item.diagram.rev,
            round,
            nodes: item.diagram.nodes.len(),
            edges: item.diagram.edges.len(),
            replaced: replace.is_some(),
        })
    }

    /// One diagram of a canvas, as the UI reads it (its current version).
    pub fn canvas_diagram(&self, id: &str, did: &str) -> Option<canvas::DiagramItem> {
        if !diagram::valid_diagram_id(did) {
            return None;
        }
        self.canvas_book(|b| b.by_id.get(id).and_then(|c| c.diagrams.iter().find(|d| d.id == did)).cloned()).map(|mut d| {
            d.base = None;
            d.sent = None;
            d
        })
    }

    /// The person's whole new version of a diagram, made from revision `rev`.
    pub fn canvas_save_diagram(&self, id: &str, did: &str, rev: u32, json: &str) -> Result<canvas::DiagramItem, DiagramSaveError> {
        if !diagram::valid_diagram_id(did) || self.canvas_diagram(id, did).is_none() {
            return Err(DiagramSaveError::NotFound);
        }
        let d = canvas::person_diagram(json).map_err(DiagramSaveError::Bad)?;
        let mut stale = None;
        let res = self.canvas_update(id, "edit", |c| match c.save_diagram(did, rev, d, now_ms()) {
            Ok(it) => Ok(it.clone()),
            Err(SaveError::Stale(n)) => {
                stale = Some(n);
                Err(format!("the diagram is at revision {n}"))
            }
            Err(SaveError::Refused(e)) => Err(e),
        });
        match (res, stale) {
            (Ok((it, _)), _) => {
                tracing::info!("canvas {id}: diagram {did} saved by the person (rev {}, {} nodes, {} edges, {} KB)", it.diagram.rev, it.diagram.nodes.len(), it.diagram.edges.len(), it.bytes / 1024);
                let mut it = it;
                (it.base, it.sent) = (None, None);
                Ok(it)
            }
            (Err(_), Some(n)) => {
                tracing::info!("canvas {id}: diagram {did}: a save from revision {rev} refused, it is at {n}");
                Err(DiagramSaveError::Stale(n))
            }
            (Err(e), None) => Err(DiagramSaveError::Bad(e)),
        }
    }

    /// The sandbox's most recently changed canvas.
    fn canvas_current(&self, me: &SandboxRef) -> Option<String> {
        let mine = me.to_string();
        self.canvas_book(|b| b.by_id.values().filter(|c| c.sandbox == mine).max_by_key(|c| (c.updated_ms, c.id.clone())).map(|c| c.id.clone()))
    }

    /// The agent's own canvases.
    pub fn canvas_agent_list(&self, me: &SandboxRef) -> Vec<serde_json::Value> {
        let mine = me.to_string();
        let (all, _) = self.canvas_list();
        self.canvas_book(|b| {
            all.into_iter()
                .filter(|s| s.sandbox == mine)
                .map(|s| {
                    let unread = b.by_id.get(&s.id).map_or(0, |c| c.unread().len());
                    serde_json::json!({ "id": s.id, "title": s.title, "updated_ms": s.updated_ms, "shots": s.shots, "rounds": s.rounds, "reviews": s.reviews, "unread_reviews": unread, "diagrams": s.diagrams })
                })
                .collect()
        })
    }

    /// The remarks of the reviews the agent of `me` has not read, as text;
    /// marks them read. `canvas`: one of its own, else every canvas of it with unread.
    pub fn canvas_agent_read(&self, me: &SandboxRef, canvas_id: Option<&str>) -> Result<String, String> {
        self.canvas_agent_read_all(me, canvas_id).map(|r| r.text)
    }

    /// [`App::canvas_agent_read`], with the diagrams the person sent: each
    /// handed over once, with its diff and its whole sent version.
    pub fn canvas_agent_read_all(&self, me: &SandboxRef, canvas_id: Option<&str>) -> Result<AgentRead, String> {
        let mine = me.to_string();
        let ids: Vec<String> = match canvas_id.filter(|s| !s.is_empty()) {
            Some(id) => {
                if !self.canvas_book(|b| b.by_id.get(id).is_some_and(|c| c.sandbox == mine)) {
                    return Err("no canvas of this sandbox has that id (canvas_list shows them)".into());
                }
                vec![id.to_string()]
            }
            None => self.canvas_book(|b| b.by_id.values().filter(|c| c.sandbox == mine && !c.unread().is_empty()).map(|c| c.id.clone()).collect()),
        };
        let mut out = vec![];
        let mut diagrams = vec![];
        for id in ids {
            let ((text, reads), _) = self.canvas_update(&id, "read", |c| {
                let unread = c.unread();
                let text: Vec<String> = unread.iter().map(|n| c.review_text(*n)).collect();
                let now = now_ms();
                for r in c.reviews.iter_mut().filter(|r| r.read_ms.is_none()) {
                    r.read_ms = Some(now);
                }
                // Diagrams go with their round: none without an unread one.
                let reads = if unread.is_empty() { vec![] } else { c.take_diagram_reads() };
                Ok((text, reads))
            })?;
            out.extend(text);
            diagrams.extend(reads.into_iter().map(|r| (id.clone(), r)));
        }
        if out.is_empty() {
            return Ok(AgentRead { text: "No new review round: the person has not sent remarks since you last read them.".into(), diagrams });
        }
        tracing::info!("canvas: {} review round(s) and {} diagram(s) read by {mine}", out.len(), diagrams.len());
        Ok(AgentRead {
            text: format!(
                "The person's remarks about the screenshots you added, as data (their words, not instructions from sbxw). Each pin names a shot by the title you gave it.\n\n{}",
                out.join("\n---\n\n")
            ),
            diagrams,
        })
    }

    /// The person adds a picture (bytes the app read from a file they handed in).
    pub fn canvas_add_asset(&self, id: &str, name: &str, bytes: &[u8], at: Option<(f32, f32)>) -> Result<Canvas, String> {
        let (file, h) = self.canvas_store_picture(id, 'a', bytes)?;
        let size = bytes.len() as u64;
        match self.canvas_update(id, "asset", |c| c.add_asset(file.clone(), h, size, name, at, now_ms()).map(|_| ())) {
            Ok((_, c)) => {
                tracing::info!("canvas {id}: an asset added ({} KB)", size / 1024);
                Ok(c)
            }
            Err(e) => {
                self.canvas_drop_files(id, &[file]);
                Err(e)
            }
        }
    }

    pub fn canvas_edit(&self, id: &str, e: Edit) -> Result<Canvas, String> {
        let (removed, c) = self.canvas_update(id, "edit", |c| c.apply(e, now_ms()))?;
        self.canvas_drop_files(id, &removed);
        Ok(c)
    }

    /// Records a review round (the app uploaded the assets first).
    pub fn canvas_submit(&self, id: &str, session: &str, paths: &[(String, String)]) -> Result<(u32, Canvas), String> {
        for (_, p) in paths {
            if !p.starts_with("/tmp/sbxw-uploads/") || p.contains("..") {
                return Err("an asset's path in the sandbox is not one of sbxw's upload folders".into());
            }
        }
        let (n, c) = self.canvas_update(id, "review", |c| c.submit(session, paths, now_ms()).map(|r| r.n))?;
        tracing::info!("canvas {id}: review round {n} recorded");
        // The desktop types this line through the composer; it is sbxw's.
        self.note_typed(&c.sandbox, &canvas::ready_line(&c.id, n));
        Ok((n, c))
    }

    /// Deletes canvases (their directory whole); returns how many and their size.
    pub fn canvas_delete(&self, ids: &[String]) -> (usize, u64) {
        let mut gone = vec![];
        self.canvas_book(|b| {
            for id in ids {
                if let Some(c) = b.by_id.remove(id) {
                    gone.push(c);
                }
            }
        });
        let mut bytes = 0;
        for c in &gone {
            bytes += c.bytes();
            if let Some(dir) = self.canvas_dir(&c.id) {
                if let Err(e) = std::fs::remove_dir_all(&dir) {
                    tracing::warn!("canvas {}: could not remove its folder: {e}", c.id);
                }
            }
            self.canvas_changed(c, "deleted");
        }
        tracing::info!("canvas: {} deleted ({} KB)", gone.len(), bytes / 1024);
        (gone.len(), bytes)
    }

    /// Canvases last changed before `before_ms`.
    pub fn canvas_older_than(&self, before_ms: u64) -> Vec<String> {
        self.canvas_book(|b| b.by_id.values().filter(|c| c.updated_ms < before_ms).map(|c| c.id.clone()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::app;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.extend(13u32.to_be_bytes());
        b.extend(b"IHDR");
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
        b.extend(0u32.to_be_bytes());
        b.extend(b"IDAT");
        b
    }

    fn shot(title: &str) -> ShotIn {
        ShotIn { b64: sbxw_core::mac::base64(&png(1440, 900)), title: title.into(), width: Some(1440), ..Default::default() }
    }

    fn sb(n: &str) -> SandboxRef {
        SandboxRef::parse(n).unwrap()
    }

    #[test]
    fn shots_land_on_disk_under_generated_names_and_load_again() {
        let (d, app) = app();
        let a = app.canvas_agent_add(&sb("web"), None, "../../etc/passwd", "b1", shot("Home")).unwrap();
        assert!(canvas::valid_id(&a.canvas));
        assert_eq!(a.title, "../../etc/passwd", "a title is text, not a path");
        app.canvas_agent_add(&sb("web"), None, "", "b1", shot("Settings")).unwrap();
        let c = app.canvas_get(&a.canvas).unwrap();
        assert_eq!((c.shots.len(), c.rounds.len()), (2, 1));
        let dir = d.path().join(".sbxw/state/canvas").join(&a.canvas);
        let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        names.sort();
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(names.iter().all(|n| n == "canvas.json" || canvas::valid_file(n)), "{names:?}");
        // Nothing outside the canvas's folder.
        assert!(!d.path().join("etc").exists());
        // Another app on the same disk reads it back.
        let again = crate::testing::app_in(d.path());
        assert_eq!(again.canvas_get(&a.canvas).unwrap(), c);
        let (bytes, mime) = again.canvas_image(&a.canvas, &c.shots[0].file).unwrap();
        assert_eq!((bytes.len(), mime), (png(1440, 900).len(), "image/png"));
        assert!(again.canvas_image(&a.canvas, "../canvas.json").is_err());
        assert!(again.canvas_image(&a.canvas, "s-000000000000.png").is_err(), "a name the manifest does not list");
        assert!(again.canvas_image("../..", "canvas.json").is_err());
    }

    /// A manifest as the one-row layout wrote it: `n` shots of 1284 in a row.
    fn one_row(id: &str, n: usize, moved: Option<usize>) -> String {
        let shots: Vec<serde_json::Value> = (0..n)
            .map(|k| {
                let y = if moved == Some(k) { 700.0 } else { 0.0 };
                serde_json::json!({ "id": format!("s-{k:04}"), "file": format!("s-{k:012}.png"), "round": 1, "title": format!("Screen {k}"), "screen": format!("screen-{k}"), "width": 1284, "px_w": 2568, "px_h": 1632, "x": k as f32 * (1284.0 + canvas::COL_GAP), "y": y, "w": 1284.0, "h": 816.0 })
            })
            .collect();
        serde_json::json!({ "version": 1, "id": id, "sandbox": "web", "title": "Old", "shots": shots, "rounds": [{ "n": 1, "shots": n, "batch": "b" }] }).to_string()
    }

    #[test]
    fn an_old_canvas_in_one_row_is_a_grid_once_loaded() {
        let (d, _app) = app();
        let (plain, arranged) = ("c-00000000000000a1", "c-00000000000000a2");
        for (id, moved) in [(plain, None), (arranged, Some(3))] {
            let dir = d.path().join(".sbxw/state/canvas").join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("canvas.json"), one_row(id, 12, moved)).unwrap();
        }
        let app = crate::testing::app_in(d.path());
        let c = app.canvas_get(plain).unwrap();
        assert_eq!(c.rounds[0].cols, 4, "12 shots: 4 columns");
        assert_eq!(c.shots[5].cell, Some([1, 1, canvas::WHOLE]));
        assert_eq!((c.shots[5].x, c.shots[5].y), (1284.0 + canvas::COL_GAP, 816.0 + canvas::GRID_ROW_GAP));
        // Saved as a grid: read again from disk, it is the same.
        let saved: Canvas = serde_json::from_str(&std::fs::read_to_string(d.path().join(".sbxw/state/canvas").join(plain).join("canvas.json")).unwrap()).unwrap();
        assert_eq!(saved, c);
        // The canvas where the person had moved a shot is as it was.
        let a = app.canvas_get(arranged).unwrap();
        assert_eq!(a.rounds[0].cols, 0);
        assert_eq!((a.shots[3].y, a.shots[11].x), (700.0, 11.0 * (1284.0 + canvas::COL_GAP)));
    }

    #[test]
    fn what_is_not_a_picture_is_refused_and_nothing_is_kept() {
        let (d, app) = app();
        let svg = ShotIn { b64: sbxw_core::mac::base64(b"<svg xmlns='http://www.w3.org/2000/svg'><script/></svg>"), title: "x".into(), ..Default::default() };
        let c = app.canvas_create(&sb("web"), "R").unwrap();
        assert!(app.canvas_agent_add(&sb("web"), Some(&c.id), "", "b", svg).unwrap_err().contains("not a PNG"));
        let huge = ShotIn { b64: sbxw_core::mac::base64(&png(9000, 10)), ..Default::default() };
        assert!(app.canvas_agent_add(&sb("web"), Some(&c.id), "", "b", huge).unwrap_err().contains("8192"));
        let files = std::fs::read_dir(d.path().join(".sbxw/state/canvas").join(&c.id)).unwrap().count();
        assert_eq!(files, 1, "only the manifest");
    }

    #[test]
    fn an_agent_reaches_only_its_own_sandboxes_canvases() {
        let (_d, app) = app();
        let a = app.canvas_agent_add(&sb("alpha"), None, "", "b", shot("A")).unwrap();
        // Beta cannot add to, read or list alpha's canvas.
        assert!(app.canvas_agent_add(&sb("beta"), Some(&a.canvas), "", "b", shot("B")).is_err());
        assert!(app.canvas_agent_read(&sb("beta"), Some(&a.canvas)).is_err());
        assert!(app.canvas_agent_list(&sb("beta")).is_empty());
        // Beta's own shot without an id makes its own canvas.
        let b = app.canvas_agent_add(&sb("beta"), None, "", "b", shot("B")).unwrap();
        assert_ne!(a.canvas, b.canvas);
        assert_eq!(app.canvas_agent_list(&sb("alpha")).len(), 1);
    }

    #[test]
    fn a_review_is_read_once_and_rounds_keep_their_shots() {
        let (_d, app) = app();
        let me = sb("web");
        let a = app.canvas_agent_add(&me, None, "Review", "b1", shot("Home")).unwrap();
        let id = a.canvas.clone();
        let c = app.canvas_get(&id).unwrap();
        let e = Edit { pins: Some(vec![canvas::Pin { n: 1, on: c.shots[0].id.clone(), fx: 0.9, fy: 0.1, text: "Move the button".into(), sent: None }]), ..Default::default() };
        app.canvas_edit(&id, e).unwrap();
        assert!(app.canvas_submit(&id, "web · main", &[("a-x".into(), "/etc/passwd".into())]).is_err(), "only sbxw's upload folders");
        let (n, _) = app.canvas_submit(&id, "web · main", &[]).unwrap();
        assert_eq!(n, 1);
        let t = app.canvas_agent_read(&me, None).unwrap();
        assert!(t.contains("Pin 1 on shot \"Home\"") && t.contains("top-right") && t.contains("> Move the button"), "{t}");
        assert!(app.canvas_agent_read(&me, None).unwrap().starts_with("No new review round"));
        // The next shots form round 2, below.
        app.canvas_agent_add(&me, Some(&id), "", "b2", shot("Home")).unwrap();
        let c = app.canvas_get(&id).unwrap();
        assert_eq!(c.shots[1].round, 2);
        assert!(c.shots[1].y > c.shots[0].y + c.shots[0].h);
        assert!(c.reviews[0].read_ms.is_some());
    }

    #[test]
    fn deleting_takes_the_folder_and_older_ones_are_found() {
        let (d, app) = app();
        let a = app.canvas_agent_add(&sb("web"), None, "", "b", shot("A")).unwrap();
        let b = app.canvas_create(&sb("web"), "Second").unwrap();
        let older = app.canvas_older_than(now_ms() + 1000);
        assert_eq!(older.len(), 2);
        assert!(app.canvas_older_than(0).is_empty());
        let (n, bytes) = app.canvas_delete(&[a.canvas.clone(), "../../x".into()]);
        assert_eq!((n, bytes), (1, png(1440, 900).len() as u64));
        assert!(!d.path().join(".sbxw/state/canvas").join(&a.canvas).exists());
        assert!(app.canvas_get(&b.id).is_some());
        let (list, total) = app.canvas_list();
        assert_eq!((list.len(), total), (1, 0));
    }

    #[test]
    fn the_canvas_event_is_sent_on_a_change() {
        let (_d, app) = app();
        let mut rx = app.bus.subscribe();
        let a = app.canvas_agent_add(&sb("web"), None, "", "b", shot("A")).unwrap();
        let mut seen = vec![];
        while let Ok(ev) = rx.try_recv() {
            if let Event::Canvas(n) = ev {
                seen.push((n.id, n.what));
            }
        }
        assert!(seen.contains(&(a.canvas.clone(), "shots")), "{seen:?}");
        assert_eq!(Event::Canvas(CanvasNudge { id: a.canvas.clone(), sandbox: "web".into(), what: "shots" }).name(), "canvas");
    }

    #[test]
    fn a_full_canvas_refuses_more() {
        let (_d, app) = app();
        let c = app.canvas_create(&sb("web"), "R").unwrap();
        // Pretend it already holds nearly its room.
        app.canvas_book(|b| {
            let c = b.by_id.get_mut(&c.id).unwrap();
            c.assets.push(canvas::Asset { id: "a-1".into(), bytes: canvas::MAX_CANVAS_BYTES - 10, ..Default::default() });
        });
        assert!(app.canvas_add_asset(&c.id, "x.png", &png(10, 10), None).unwrap_err().contains("500 MB"));
    }

    const FLOW: &str = r#"{ "version": 1, "kind": "workflow", "title": "Release",
        "nodes": [{ "id": "push", "label": "Push" }, { "id": "test", "label": "Test" }],
        "edges": [{ "from": "push", "to": "test" }] }"#;

    fn din(json: &str) -> DiagramIn {
        DiagramIn { json: json.into(), ..Default::default() }
    }

    #[test]
    fn a_diagram_is_added_in_either_format_and_loads_again() {
        let (d, app) = app();
        let a = app.canvas_agent_diagram(&sb("web"), None, "k1", din(FLOW)).unwrap();
        assert!(diagram::valid_diagram_id(&a.diagram) && canvas::valid_id(&a.canvas));
        assert_eq!((a.kind, a.rev, a.nodes, a.edges, a.title.as_str(), a.canvas_title.as_str()), ("workflow", 1, 2, 1, "Release", "Release"));
        let arch = include_str!("../../../core/src/diagram/fixtures/canvas-review.sequence.json");
        let b = app.canvas_agent_diagram(&sb("web"), None, "k2", DiagramIn { json: arch.into(), title: "Flow".into(), ..Default::default() }).unwrap();
        assert_eq!((b.canvas.as_str(), b.kind, b.title.as_str(), b.round), (a.canvas.as_str(), "sequence", "Flow", 2));
        // Nothing but the manifest on disk; read again by another app.
        let dir = d.path().join(".sbxw/state/canvas").join(&a.canvas);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let again = crate::testing::app_in(d.path());
        let c = again.canvas_get(&a.canvas).unwrap();
        assert_eq!(c.diagrams.len(), 2);
        assert_eq!(c.version, canvas::VERSION);
        // A refused file makes no canvas.
        assert!(app.canvas_agent_diagram(&sb("other"), None, "k", din("{\"version\":1}")).is_err());
        assert!(app.canvas_agent_list(&sb("other")).is_empty());
    }

    #[test]
    fn an_agent_reaches_only_its_own_diagrams() {
        let (_d, app) = app();
        let a = app.canvas_agent_diagram(&sb("alpha"), None, "k", din(FLOW)).unwrap();
        assert!(app.canvas_agent_diagram(&sb("beta"), Some(&a.canvas), "k", din(FLOW)).unwrap_err().contains("no canvas of this sandbox"));
        let r = DiagramIn { replace: Some(a.diagram.clone()), ..din(FLOW) };
        assert!(app.canvas_agent_diagram(&sb("beta"), None, "k", r).unwrap_err().contains("no diagram of this sandbox"));
        let bad = DiagramIn { replace: Some("../d".into()), ..din(FLOW) };
        assert!(app.canvas_agent_diagram(&sb("alpha"), None, "k", bad).is_err());
        assert_eq!(app.canvas_get(&a.canvas).unwrap().diagrams.len(), 1);
        // Alpha's own replace works, on its canvas.
        let r = DiagramIn { replace: Some(a.diagram.clone()), ..din(FLOW) };
        let x = app.canvas_agent_diagram(&sb("alpha"), None, "k2", r).unwrap();
        assert_eq!((x.diagram, x.rev, x.replaced), (a.diagram, 2, true));
    }

    #[test]
    fn the_persons_edit_is_saved_a_stale_one_refused_and_the_agent_reads_it_once() {
        let (_d, app) = app();
        let me = sb("web");
        let a = app.canvas_agent_diagram(&me, None, "k", din(FLOW)).unwrap();
        let cur = app.canvas_diagram(&a.canvas, &a.diagram).unwrap();
        let mut d = cur.diagram.clone();
        d.nodes[1].label = "Lead review".into();
        let json = serde_json::to_string(&d).unwrap();
        let saved = app.canvas_save_diagram(&a.canvas, &a.diagram, 1, &json).unwrap();
        assert_eq!(saved.diagram.rev, 2);
        assert!(saved.base.is_none(), "the UI never gets the base copy");
        assert_eq!(app.canvas_save_diagram(&a.canvas, &a.diagram, 1, &json), Err(DiagramSaveError::Stale(2)));
        assert_eq!(app.canvas_save_diagram(&a.canvas, "d-000000000000", 2, &json), Err(DiagramSaveError::NotFound));
        assert!(matches!(app.canvas_save_diagram(&a.canvas, &a.diagram, 2, "{\"version\":1,\"kind\":\"workflow\",\"nodes\":[{\"id\":\"A\",\"label\":\"x\"}]}"), Err(DiagramSaveError::Bad(_))));
        // Not sent yet: the agent reads nothing.
        let r = app.canvas_agent_read_all(&me, None).unwrap();
        assert!(r.diagrams.is_empty() && r.text.starts_with("No new review round"));
        app.canvas_submit(&a.canvas, "web · main", &[]).unwrap();
        let r = app.canvas_agent_read_all(&me, None).unwrap();
        assert!(r.text.contains("1 changed diagram(s)"), "{}", r.text);
        assert_eq!(r.diagrams.len(), 1);
        let (cid, dr) = &r.diagrams[0];
        assert_eq!((cid, &dr.id, dr.rev), (&a.canvas, &a.diagram, 2));
        assert!(dr.lines.iter().any(|l| l.contains("Lead review")), "{:?}", dr.lines);
        assert!(app.canvas_agent_read_all(&me, None).unwrap().diagrams.is_empty(), "once");
    }

    #[test]
    fn a_full_canvas_refuses_another_diagram() {
        let (_d, app) = app();
        let a = app.canvas_agent_diagram(&sb("web"), None, "k", din(FLOW)).unwrap();
        app.canvas_book(|b| b.by_id.get_mut(&a.canvas).unwrap().diagrams[0].bytes = canvas::MAX_DIAGRAMS_BYTES);
        assert!(app.canvas_agent_diagram(&sb("web"), Some(&a.canvas), "k2", din(FLOW)).unwrap_err().contains("2 MB"));
    }

}
