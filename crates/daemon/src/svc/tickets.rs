//! Tickets on the host's disk (the person's data), per sandbox:
//! `state/tickets/<sandbox>/tickets.json` and its files under
//! `state/tickets/<sandbox>/files/`, each under a name generated here. The
//! rules and the board are in `sbxw_core::tickets`; this file keeps the
//! books, follows the plans, serves the board and hands a ticket to an agent.
//!
//! * A path is only ever built from a parsed sandbox name and a checked,
//!   generated file name the book lists ([`tickets::valid_file`]). Nothing a
//!   person or an agent sends is joined to a directory.
//! * Writes are atomic (a private temporary file and a rename), the folders
//!   0700, the files 0600.
//! * The log gets numbers, counts and sizes, never a title, a description
//!   or a file name.
//! * An agent reaches only its own sandbox's tickets (the token's sandbox,
//!   never a body field) and only reads them. Nothing here writes a ticket
//!   for an agent.
//! * The column of a card is decided in one place, `tickets::column_of`,
//!   from the plans as they are and, for a plan already pruned, from what the
//!   ticket recorded last.

use crate::app::{now_ms, App};
use crate::bus::Event;
use sbxw_core::plans::Plan;
use sbxw_core::team;
use sbxw_core::tickets::{self, Book, Column, Draft, Move, Ticket, TicketFile};
use sbxw_core::SandboxRef;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// The books, each read from disk the first time its sandbox is asked for.
#[derive(Default)]
pub struct Store {
    listed: bool,
    books: HashMap<String, Book>,
}

/// What the `tickets` event says: which sandbox's board changed.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct TicketsNudge {
    pub sandbox: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// `created`, `edited`, `file`, `moved`, `deleted`, `followed`, `sent`.
    pub what: &'static str,
}

/// How "Send to the lead" went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Sent {
    /// Typed now.
    Typed,
    /// Held: the lead waits on a question; typed once it is idle or at work.
    Held,
}

impl App {
    fn tickets_root(&self) -> PathBuf {
        self.store.paths.state().join("tickets")
    }

    /// A parsed sandbox's folder (its text form: `web`, `host~web`).
    fn tickets_dir(&self, sb: &str) -> Option<PathBuf> {
        let r = SandboxRef::parse(sb).ok()?;
        let name = r.to_string();
        (name == sb && !name.contains(['/', '\\']) && !name.starts_with('.')).then(|| self.tickets_root().join(name))
    }

    fn tickets_load(&self, sb: &str) -> Book {
        let Some(dir) = self.tickets_dir(sb) else { return Book::new(sb) };
        match std::fs::read_to_string(dir.join("tickets.json")) {
            Ok(t) => match serde_json::from_str::<Book>(&t) {
                Ok(mut b) if b.sandbox == sb => {
                    b.version = tickets::VERSION;
                    b.next = b.next.max(b.tickets.iter().map(|t| t.n).max().unwrap_or(0) + 1);
                    b
                }
                Ok(_) => {
                    tracing::warn!("tickets of {sb}: the book names another sandbox; started empty");
                    Book::new(sb)
                }
                Err(e) => {
                    tracing::warn!("tickets of {sb}: unreadable, started empty (the file is left as it was): {e}");
                    let _ = std::fs::rename(dir.join("tickets.json"), dir.join(format!("tickets.unreadable-{}.json", now_ms())));
                    Book::new(sb)
                }
            },
            Err(_) => Book::new(sb),
        }
    }

    /// Runs `f` on the sandboxes' books (loaded on first use, following the
    /// plans kept then, so a change while the daemon was down is caught up).
    fn tickets_with<R>(&self, sb: &str, f: impl FnOnce(&mut Book) -> R) -> R {
        let fresh = !self.tickets.lock().unwrap().books.contains_key(sb);
        if fresh {
            let mut b = self.tickets_load(sb);
            let plans = self.plans.lock().unwrap().of(sb);
            let now = now_ms();
            let mut changed = false;
            for p in &plans {
                changed |= b.follow(p, now);
                changed |= b.record_threads(p, now);
            }
            if changed {
                let _ = self.tickets_save(&b);
            }
            self.tickets.lock().unwrap().books.entry(sb.to_string()).or_insert(b);
        }
        let mut g = self.tickets.lock().unwrap();
        f(g.books.get_mut(sb).expect("loaded above"))
    }

    /// Every sandbox that has a book on disk or in memory.
    fn tickets_sandboxes(&self) -> Vec<String> {
        let listed = self.tickets.lock().unwrap().listed;
        if !listed {
            let names: Vec<String> = std::fs::read_dir(self.tickets_root())
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| self.tickets_dir(n).is_some())
                .collect();
            for n in names {
                self.tickets_with(&n, |_| ());
            }
            self.tickets.lock().unwrap().listed = true;
        }
        let mut v: Vec<String> = self.tickets.lock().unwrap().books.keys().cloned().collect();
        v.sort();
        v
    }

    fn tickets_save(&self, b: &Book) -> Result<(), String> {
        let dir = self.tickets_dir(&b.sandbox).ok_or("bad sandbox name")?;
        let body = serde_json::to_vec_pretty(b).map_err(|e| e.to_string())?;
        crate::store::write_atomic(&dir.join("tickets.json"), &body).map_err(|e| format!("could not save the tickets: {e:#}"))
    }

    fn tickets_changed(&self, sb: &str, n: Option<u32>, what: &'static str) {
        self.send(Event::Tickets(TicketsNudge { sandbox: sb.to_string(), n, what }));
    }

    /// Changes `sb`'s book with `f` and saves it; `Err` leaves it as it was.
    fn tickets_update<R>(&self, sb: &str, n: Option<u32>, what: &'static str, f: impl FnOnce(&mut Book) -> Result<R, String>) -> Result<R, String> {
        let r = self.tickets_with(sb, |b| {
            let mut next = b.clone();
            let r = f(&mut next)?;
            self.tickets_save(&next)?;
            *b = next;
            Ok::<_, String>(r)
        })?;
        self.tickets_changed(sb, n, what);
        Ok(r)
    }

    fn plans_of(&self, sb: Option<&str>) -> Vec<Plan> {
        let p = self.plans.lock().unwrap();
        match sb {
            Some(s) => p.of(s),
            None => p.list(),
        }
    }

    /// The column a ticket is in now.
    fn ticket_column(&self, sb: &str, t: &Ticket) -> Column {
        let plans = self.plans_of(Some(sb));
        let by: HashMap<&str, &Plan> = plans.iter().map(|p| (p.id.as_str(), p)).collect();
        tickets::ticket_column(t, &by).0
    }

    // ── The board ────────────────────────────────────────────────────────

    /// The board of one sandbox, or of every sandbox (`None`).
    pub fn tickets_board(&self, sb: Option<&SandboxRef>, older: bool) -> tickets::Board {
        let names: Vec<String> = match sb {
            Some(s) => vec![s.to_string()],
            None => {
                let mut v = self.tickets_sandboxes();
                for p in self.plans_of(None) {
                    if !v.contains(&p.sandbox) {
                        v.push(p.sandbox.clone());
                    }
                }
                v
            }
        };
        let books: Vec<Book> = names.iter().map(|n| self.tickets_with(n, |b| b.clone())).collect();
        let plans: Vec<Plan> = self.plans_of(None).into_iter().filter(|p| names.contains(&p.sandbox)).collect();
        let refs: Vec<&Book> = books.iter().collect();
        tickets::board(&sb.map_or("all".to_string(), |s| s.to_string()), &refs, &plans, now_ms(), older)
    }

    pub fn ticket_view(&self, sb: &SandboxRef, n: u32) -> Option<Value> {
        let s = sb.to_string();
        let t = self.tickets_with(&s, |b| b.get(n).cloned())?;
        let plans = self.plans_of(Some(&s));
        let log = t.link.as_ref().map(|l| self.plans.lock().unwrap().log_of(&l.plan));
        Some(tickets::ticket_view(&s, &t, &plans, log.as_ref(), now_ms()))
    }

    // ── The person's acts ────────────────────────────────────────────────

    pub fn ticket_create(&self, sb: &SandboxRef, d: Draft) -> Result<Value, String> {
        let s = sb.to_string();
        let n = self.tickets_update(&s, None, "created", |b| b.create(d, now_ms()).map(|t| t.n))?;
        tracing::info!("tickets: {} created in {s}", tickets::num(n));
        self.ticket_view(sb, n).ok_or_else(|| "the ticket went".into())
    }

    /// Title, description, priority; `validated` on a merged ticket.
    pub fn ticket_edit(&self, sb: &SandboxRef, n: u32, d: Draft, validated: Option<bool>) -> Result<Value, String> {
        let s = sb.to_string();
        let column = self.tickets_with(&s, |b| b.get(n).cloned()).map(|t| self.ticket_column(&s, &t)).ok_or_else(|| format!("no ticket {}", tickets::num(n)))?;
        let source = self.tickets_with(&s, |b| b.get(n).map(|t| t.source));
        let texts = d.title.is_some() || d.description.is_some() || d.priority.is_some();
        if texts && source == Some(tickets::Source::Thread) {
            return Err("a card recorded from a thread is not edited: only its validation changes".into());
        }
        self.tickets_update(&s, Some(n), "edited", |b| {
            if texts {
                b.edit(n, d, now_ms())?;
            }
            if let Some(v) = validated {
                b.validate(n, v, column, now_ms())?;
            }
            Ok(())
        })?;
        tracing::info!("tickets: {} edited in {s}", tickets::num(n));
        self.ticket_view(sb, n).ok_or_else(|| "the ticket went".into())
    }

    /// The person attaches a file (bytes the app read from a file they handed in).
    pub fn ticket_attach(&self, sb: &SandboxRef, n: u32, name: &str, bytes: &[u8]) -> Result<Value, String> {
        let s = sb.to_string();
        let kind = tickets::sniff(bytes)?;
        let size = bytes.len() as u64;
        self.tickets_with(&s, |b| b.room(n, size))?;
        let file = tickets::new_file(kind);
        let dir = self.tickets_dir(&s).ok_or("bad sandbox name")?.join("files");
        crate::store::write_atomic(&dir.join(&file), bytes).map_err(|e| format!("could not store the file: {e:#}"))?;
        let f = TicketFile { id: tickets::new_file_id(), file: file.clone(), name: tickets::shown_name(name), bytes: size, kind, added_ms: now_ms() };
        match self.tickets_update(&s, Some(n), "file", |b| b.add_file(n, f, now_ms()).map(|_| ())) {
            Ok(()) => {
                tracing::info!("tickets: a file attached to {} in {s} ({} KB, {:?})", tickets::num(n), size / 1024, kind);
                self.ticket_view(sb, n).ok_or_else(|| "the ticket went".into())
            }
            Err(e) => {
                let _ = std::fs::remove_file(dir.join(&file));
                Err(e)
            }
        }
    }

    /// A file's bytes, type and shown name, by an id the book lists.
    pub fn ticket_file(&self, sb: &SandboxRef, n: u32, id: &str) -> Result<(Vec<u8>, &'static str, String), String> {
        let s = sb.to_string();
        if !tickets::valid_file_id(id) {
            return Err("no such file".into());
        }
        let f = self.tickets_with(&s, |b| b.get(n).and_then(|t| t.file(id)).cloned()).ok_or("no such file")?;
        if !tickets::valid_file(&f.file) {
            return Err("no such file".into());
        }
        let path = self.tickets_dir(&s).ok_or("no such file")?.join("files").join(&f.file);
        let bytes = std::fs::read(path).map_err(|_| "the file is missing on disk".to_string())?;
        Ok((bytes, f.kind.mime(), f.name))
    }

    pub fn ticket_remove_file(&self, sb: &SandboxRef, n: u32, id: &str) -> Result<Value, String> {
        let s = sb.to_string();
        let file = self.tickets_update(&s, Some(n), "file", |b| b.remove_file(n, id, now_ms()))?;
        if tickets::valid_file(&file) {
            if let Some(d) = self.tickets_dir(&s) {
                let _ = std::fs::remove_file(d.join("files").join(&file));
            }
        }
        tracing::info!("tickets: a file removed from {} in {s}", tickets::num(n));
        self.ticket_view(sb, n).ok_or_else(|| "the ticket went".into())
    }

    pub fn ticket_move(&self, sb: &SandboxRef, n: u32, m: Move) -> Result<(), String> {
        let s = sb.to_string();
        let plans = self.plans_of(Some(&s));
        let by: HashMap<&str, &Plan> = plans.iter().map(|p| (p.id.as_str(), p)).collect();
        let live = |t: &Ticket| tickets::ticket_column(t, &by).0;
        self.tickets_update(&s, Some(n), "moved", |b| b.move_ticket(n, m, &live))
    }

    /// Deletes a ticket and its files; returns how many files and their size.
    /// Refused while its thread is at work.
    pub fn ticket_delete(&self, sb: &SandboxRef, n: u32) -> Result<(usize, u64), String> {
        let s = sb.to_string();
        let plans = self.plans_of(Some(&s));
        let by: HashMap<&str, &Plan> = plans.iter().map(|p| (p.id.as_str(), p)).collect();
        let t = self.tickets_with(&s, |b| b.get(n).cloned()).ok_or_else(|| format!("no ticket {}", tickets::num(n)))?;
        let (col, marks, _) = tickets::ticket_column(&t, &by);
        let working = t.link.is_some() && matches!(col, Column::Progress | Column::Test | Column::Ready) && !marks.contains(&tickets::Mark::TeamEnded);
        if working {
            return Err("its thread is at work: delete it once merged, or once its plan is refused".into());
        }
        let gone = self.tickets_update(&s, Some(n), "deleted", |b| b.delete(n))?;
        if let Some(d) = self.tickets_dir(&s) {
            for f in gone.files.iter().filter(|f| tickets::valid_file(&f.file)) {
                let _ = std::fs::remove_file(d.join("files").join(&f.file));
            }
        }
        tracing::info!("tickets: {} deleted in {s} ({} files, {} KB)", tickets::num(n), gone.files.len(), gone.bytes() / 1024);
        Ok((gone.files.len(), gone.bytes()))
    }

    /// "Send to the lead": ONE fixed line, written here from the number
    /// alone, into the sandbox's main session, under a team note's delivery
    /// rule (typed when idle or working, held while it waits on a question
    /// or a prompt, refused when no session is open). The ticket's words
    /// are never typed. The app asks the person first (its Confirm).
    pub fn ticket_send(self: &Arc<Self>, sb: &SandboxRef, n: u32) -> Result<Sent, String> {
        let s = sb.to_string();
        let t = self.tickets_with(&s, |b| b.get(n).cloned()).ok_or_else(|| format!("no ticket {}", tickets::num(n)))?;
        if t.source != tickets::Source::Person {
            return Err("a card recorded from a thread is not sent".into());
        }
        if t.link.is_some() {
            return Err("already in a plan".into());
        }
        let line = tickets::send_line(n);
        let who = vec![team::LEAD.to_string()];
        let d = self.note_targets(sb, &who, |k| self.sessions.is_live(k)).into_iter().next().map(|(_, d)| d).unwrap_or(team::Delivery::Drop);
        let sent = match d {
            team::Delivery::Drop => return Err(format!("no session of {s}'s agent is open: open its terminal first")),
            team::Delivery::Type => {
                self.spawn_push(sb.clone(), sbxw_core::session::SessionKind::Agent, line, None);
                Sent::Typed
            }
            team::Delivery::Hold => {
                self.ticket_hold_line(sb.clone(), line);
                Sent::Held
            }
        };
        let _ = self.tickets_update(&s, Some(n), "sent", |b| {
            if let Some(t) = b.tickets.iter_mut().find(|t| t.n == n) {
                t.history.push(tickets::Step { at: now_ms(), kind: "sent".into(), text: "Sent to the lead".into() });
            }
            Ok(())
        });
        tracing::info!("tickets: {} sent to the lead of {s} ({sent:?})", tickets::num(n));
        Ok(sent)
    }

    /// The line waits while the lead waits on a question or a prompt (the
    /// team note's rule, `note_targets`), and is typed once it is idle or at
    /// work again; dropped if the session ends, or after 30 minutes.
    fn ticket_hold_line(self: &Arc<Self>, sb: SandboxRef, line: String) {
        let app = self.clone();
        tokio::spawn(async move {
            let mut rx = app.bus.subscribe();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30 * 60);
            let who = vec![team::LEAD.to_string()];
            loop {
                let d = app.note_targets(&sb, &who, |k| app.sessions.is_live(k)).into_iter().next().map(|(_, d)| d).unwrap_or(team::Delivery::Drop);
                match d {
                    team::Delivery::Type => {
                        app.spawn_push(sb.clone(), sbxw_core::session::SessionKind::Agent, line.clone(), None);
                        return;
                    }
                    team::Delivery::Drop => return,
                    team::Delivery::Hold => {}
                }
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    return;
                }
                match tokio::time::timeout(left, rx.recv()).await {
                    Ok(Ok(Event::Session(i))) if i.sandbox == sb.to_string() => {}
                    Ok(Ok(_)) | Ok(Err(RecvError::Lagged(_))) => continue,
                    _ => return,
                }
            }
        });
    }

    // ── The plans ────────────────────────────────────────────────────────

    /// Follows every plan change: links, columns, refusals, merged threads.
    pub fn start_tickets_listener(self: &Arc<Self>) {
        let app = self.clone();
        let mut rx = self.bus.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(Event::Plan(p)) => app.tickets_follow(&p),
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }

    pub(crate) fn tickets_follow(&self, p: &Plan) {
        let fresh = !self.tickets.lock().unwrap().books.contains_key(&p.sandbox);
        let has_file = self.tickets_dir(&p.sandbox).is_some_and(|d| d.join("tickets.json").exists());
        let names_ticket = p.threads.iter().any(|t| t.ticket.is_some());
        let merged = p.merge.as_ref().is_some_and(|m| m.results.iter().any(|r| r.ok) || m.state == sbxw_core::plans::MergeState::Merged);
        if fresh && !has_file && !names_ticket && !merged {
            // Nothing to follow: no book is made for a sandbox that has none.
            self.tickets_changed(&p.sandbox, None, "followed");
            return;
        }
        let now = now_ms();
        let res = self.tickets_with(&p.sandbox, |b| {
            let mut next = b.clone();
            let changed = next.follow(p, now) | next.record_threads(p, now);
            if changed {
                self.tickets_save(&next)?;
                *b = next;
            }
            Ok::<_, String>(changed)
        });
        match res {
            Ok(_) => self.tickets_changed(&p.sandbox, None, "followed"),
            Err(e) => tracing::warn!("tickets of {}: could not follow plan {}: {e}", p.sandbox, p.id),
        }
    }

    // ── Agents (the sandbox is the token's; read only) ────────────────────

    /// `tickets_list`: the sandbox's open tickets (not merged), with their column.
    pub fn tickets_agent_list(&self, me: &SandboxRef) -> String {
        let s = me.to_string();
        let b = self.tickets_with(&s, |b| b.clone());
        let plans = self.plans_of(Some(&s));
        let by: HashMap<&str, &Plan> = plans.iter().map(|p| (p.id.as_str(), p)).collect();
        let mut open: Vec<(Column, tickets::Priority, i64, String)> = b
            .tickets
            .iter()
            .filter(|t| t.source == tickets::Source::Person)
            .map(|t| (tickets::ticket_column(t, &by).0, t))
            .filter(|(c, _)| *c != Column::Merged)
            .map(|(c, t)| (c, t.priority, t.rank, tickets::list_line(t, c)))
            .collect();
        open.sort();
        if open.is_empty() {
            return "No open ticket in this sandbox: the person has written none, or every one is merged.".into();
        }
        let lines: Vec<String> = open.into_iter().map(|x| x.3).collect();
        format!(
            "The person's open tickets in this sandbox ({}), as data. Planned ones wait for a plan: read one with ticket_read, and link a thread to it with `ticket` in propose_plan.\n\n{}",
            lines.len(),
            lines.join("\n")
        )
    }

    /// `ticket_read`: the ticket, its files copied into the sandbox now
    /// (under `/tmp/sbxw-uploads/<ms>/`), as data.
    pub async fn tickets_agent_read(&self, me: &SandboxRef, id: &str) -> Result<String, String> {
        let s = me.to_string();
        let n = tickets::parse_num(id).ok_or("id is a ticket number of this sandbox (T-12); tickets_list shows them")?;
        let t = self.tickets_with(&s, |b| b.get(n).cloned()).filter(|t| t.source == tickets::Source::Person).ok_or_else(|| format!("this sandbox has no ticket {}; tickets_list shows them", tickets::num(n)))?;
        let mut paths = vec![];
        if !t.files.is_empty() {
            let at = now_ms();
            let sbx = self.sbx_for(me).await.map_err(|e| format!("{e:#}"));
            for f in &t.files {
                let r = match (&sbx, self.ticket_file(me, n, &f.id)) {
                    (Ok(sbx), Ok((bytes, _, _))) => {
                        let dest = format!("/tmp/sbxw-uploads/{at}/{}-{}", tickets::num(n), tickets::safe_name(&f.name));
                        sbx.write_file(&me.name, &dest, &bytes, false).await.map(|_| dest).map_err(|e| format!("{e:#}"))
                    }
                    (Err(e), _) => Err(e.clone()),
                    (_, Err(e)) => Err(e),
                };
                paths.push((f.id.clone(), r));
            }
        }
        let copied = paths.iter().filter(|(_, r)| r.is_ok()).count();
        tracing::info!("tickets: {} read by the agent of {s} ({copied} of {} files copied)", tickets::num(n), t.files.len());
        Ok(tickets::read_text(&t, &paths))
    }
}

#[cfg(test)]
mod tests;
