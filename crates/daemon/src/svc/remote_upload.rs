//! A paired phone hands a file to a leader (API.md 2.6.1,
//! `docs/security.md#remote-access`). The bytes are streamed to a spool file
//! on the Mac, the cap counted on every chunk; the phone gets an opaque id
//! bound to it and to that leader. A message naming the id copies the file
//! into the sandbox with the web UI's own writer ([`App::write_upload`]) and
//! types the block of paths after the person's text; the id is spent only
//! when the Return was sent.
//!
//! Never logged, never journaled: a file's name, path or content. Log lines
//! and journal entries name a device and a size class.

use super::remote_access::{DeviceCtx, Reason, Refusal};
use super::remote_sent::{Sent, SentStore};
use crate::app::{now_ms, App};
use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::remote_access::uploads::{self as rules, SizeClass, Unresolved, Upload, FILE_MAX};
use sbxw_core::SandboxRef;
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// The spool folder, under `<sbxw home>/remote/`.
pub const SPOOL_DIR: &str = "uploads";
/// The whole body must arrive within this (the sidecar cuts at 120 s).
pub const BODY_DEADLINE: Duration = Duration::from_secs(180);
/// And no gap between two chunks longer than this.
pub const CHUNK_IDLE: Duration = Duration::from_secs(30);
/// Bytes read before the kind is judged (tar's mark is at 257).
const HEAD: usize = 512;
/// The longest `name` hint taken.
pub const NAME_HINT_MAX: usize = 1024;

/// What the phone is told of its upload: never a path.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct UploadView {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub expires_ms: u64,
}

/// Remove the spool files of uploads that went.
pub(crate) fn remove_spools(remote_dir: &Path, gone: Vec<Upload>) {
    for u in gone {
        let _ = std::fs::remove_file(remote_dir.join(SPOOL_DIR).join(&u.spool));
    }
}

/// While a device uploads: no second upload from it. Released on drop.
struct Slot {
    app: Arc<App>,
    device: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.app.remote_access.lock().unwrap().uploading.remove(&self.device);
    }
}

/// A spool file being written: removed on drop unless kept.
struct Spool {
    path: std::path::PathBuf,
    keep: bool,
}

impl Drop for Spool {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn reason(r: Reason) -> Refusal {
    Refusal::Reason(r)
}

/// A new spool file, `0600` in a `0700` folder, under a random name.
async fn open_spool(remote_dir: &Path) -> std::io::Result<(tokio::fs::File, Spool, String)> {
    let dir = remote_dir.join(SPOOL_DIR);
    std::fs::create_dir_all(&dir)?;
    let mut open = tokio::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(remote_dir, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        open.mode(0o600);
    }
    let name = format!("{}.bin", sbxw_core::ids::random_hex(16));
    let path = dir.join(&name);
    let f = open.open(&path).await?;
    Ok((f, Spool { path, keep: false }, name))
}

impl App {
    /// `POST /r/leaders/:sandbox/upload`: stream `body` to a spool file and
    /// keep a handle for this device and leader. `sb` is a current leader
    /// (the route checked). `declared` is the `Content-Length`, if any: a
    /// hint to refuse early, never trusted to stop reading.
    pub async fn remote_upload<S, E>(self: &Arc<Self>, d: &DeviceCtx, sb: &SandboxRef, name_hint: &str, declared: Option<u64>, body: S) -> Result<UploadView, Refusal>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
    {
        let key = sb.to_string();
        let (dir, idle, _slot) = {
            let mut rt = self.remote_access.lock().unwrap();
            let now = now_ms();
            // Every attempt counts, against this device only.
            rt.upload_limit.hit(&d.id, now).map_err(Refusal::Limited)?;
            if name_hint.trim().is_empty() || name_hint.len() > NAME_HINT_MAX {
                return Err(reason(Reason::Name));
            }
            if declared.is_some_and(|n| n > FILE_MAX) {
                return Err(reason(Reason::TooLarge));
            }
            if declared == Some(0) {
                return Err(reason(Reason::Empty));
            }
            if rt.uploads.room(&d.id, &key, declared.unwrap_or(0)).is_err() {
                return Err(reason(Reason::TooMany));
            }
            if !rt.uploading.insert(d.id.clone()) {
                return Err(reason(Reason::Uploading));
            }
            (rt.dir().to_path_buf(), rt.upload_idle, Slot { app: self.clone(), device: d.id.clone() })
        };
        let (mut file, mut spool, spool_name) = open_spool(&dir).await.map_err(|e| {
            tracing::warn!("remote access: cannot open an upload spool file: {e}");
            Refusal::Failed("the Mac could not store the file".into())
        })?;
        let size = match tokio::time::timeout(BODY_DEADLINE, read_into(&mut file, body, idle)).await {
            Ok(r) => r?,
            Err(_) => return Err(reason(Reason::Timeout)),
        };
        let name = sbxw_core::uploads::upload_name(name_hint);
        let now = now_ms();
        let view = UploadView { id: rules::new_id(), name: name.clone(), size, expires_ms: now + rules::TTL_MS };
        let up = Upload { id: view.id.clone(), device_id: d.id.clone(), sandbox: key, name, size, expires_ms: view.expires_ms, spool: spool_name, copied: None };
        {
            let mut rt = self.remote_access.lock().unwrap();
            // Revoked or turned off while the body came: nothing kept.
            if !rt.has_device(&d.id) {
                return Err(Refusal::NotFound);
            }
            if rt.uploads.add(up).is_err() {
                return Err(reason(Reason::TooMany));
            }
            spool.keep = true;
            rt.record_sized(&d.id, &d.name, Kind::Upload, Some(SizeClass::of(size)));
        }
        tracing::info!("remote access: device {}: upload ({:?})", d.id, SizeClass::of(size));
        Ok(view)
    }

    /// `DELETE /r/leaders/:sandbox/upload/:id`.
    pub fn remote_upload_drop(&self, d: &DeviceCtx, sb: &SandboxRef, id: &str) -> Result<(), Refusal> {
        let mut rt = self.remote_access.lock().unwrap();
        let gone = rt.uploads.drop_one(&d.id, &sb.to_string(), id, now_ms()).ok_or(Refusal::NotFound)?;
        remove_spools(rt.dir(), vec![gone]);
        drop(rt);
        tracing::info!("remote access: device {}: upload dropped", d.id);
        Ok(())
    }

    /// The uploads a message names, or why not.
    pub(crate) fn remote_resolve_attachments(&self, d: &DeviceCtx, sb: &SandboxRef, ids: &[String]) -> Result<Vec<Upload>, Refusal> {
        let rt = self.remote_access.lock().unwrap();
        rt.uploads.resolve(&d.id, &sb.to_string(), ids, now_ms()).map_err(|e| match e {
            Unresolved::TooMany => reason(Reason::TooManyIds),
            Unresolved::Bad => reason(Reason::Attachment),
        })
    }

    /// Copy each named file into the sandbox (once: a retry reuses the
    /// path), one copy at a time for the whole daemon; the sandbox paths, in
    /// the message's order.
    pub(crate) async fn remote_copy_attachments(&self, d: &DeviceCtx, sb: &SandboxRef, ids: &[String]) -> Result<Vec<String>, Refusal> {
        let lock = self.remote_access.lock().unwrap().copy_lock.clone();
        let _one_copy = lock.lock().await;
        let ups = self.remote_resolve_attachments(d, sb, ids)?;
        let dir = self.remote_access.lock().unwrap().dir().join(SPOOL_DIR);
        let fresh: Vec<&Upload> = ups.iter().filter(|u| u.copied.is_none()).collect();
        let names = sbxw_core::uploads::distinct_names(&fresh.iter().map(|u| u.name.clone()).collect::<Vec<_>>());
        let at = now_ms();
        for (u, name) in fresh.iter().zip(names) {
            let bytes = tokio::fs::read(dir.join(&u.spool)).await.map_err(|_| reason(Reason::Attachment))?;
            match self.write_upload(sb, at, &name, &bytes).await {
                Ok(path) => self.remote_access.lock().unwrap().uploads.set_copied(&u.id, &path),
                Err(e) => {
                    tracing::warn!("remote access: device {}: a file could not be copied into the sandbox", d.id);
                    tracing::debug!("remote access: copy into '{sb}' failed: {e:#}");
                    return Err(Refusal::Failed("the file could not be copied into the sandbox; it is kept, try again".into()));
                }
            }
        }
        let ups = self.remote_resolve_attachments(d, sb, ids)?;
        ups.into_iter().map(|u| u.copied.ok_or_else(|| reason(Reason::Attachment))).collect()
    }

    /// The message was submitted: its ids are spent, their spool files go.
    pub(crate) fn remote_spend_attachments(&self, ids: &[String]) {
        let (gone, dir) = {
            let mut rt = self.remote_access.lock().unwrap();
            (rt.uploads.spend(ids), rt.dir().to_path_buf())
        };
        // Keep a copy of each image before its spool goes (the copy runs
        // outside the lock: a file can be 25 MiB).
        let now = now_ms();
        let kept: Vec<Sent> = gone
            .iter()
            .filter_map(|u| SentStore::copy_in(&dir, &dir.join(SPOOL_DIR).join(&u.spool), &u.device_id, &u.sandbox, u.copied.as_deref()?, now))
            .collect();
        let mut rt = self.remote_access.lock().unwrap();
        for s in kept {
            // Revoked while the copy ran: not kept.
            if rt.has_device(&s.device_id) {
                rt.sent.add(s);
            } else {
                let _ = std::fs::remove_file(rt.sent.dir().join(format!("{}.{}", s.id, s.kind.ext())));
            }
        }
        remove_spools(rt.dir(), gone);
    }

    /// `GET /r/sent/:id`: the bytes of an image this device sent, and its
    /// content type. Not found for another device's id, an unknown or an
    /// expired one, or a sandbox that is not reachable now.
    pub async fn remote_sent_image(&self, d: &DeviceCtx, id: &str) -> Result<(Vec<u8>, &'static str), Refusal> {
        let (sandbox, path, mime) = self.remote_access.lock().unwrap().sent.get(&d.id, id, now_ms()).ok_or(Refusal::NotFound)?;
        self.remote_target(&sandbox)?;
        let bytes = tokio::fs::read(path).await.map_err(|_| Refusal::NotFound)?;
        Ok((bytes, mime))
    }

    /// Unsent uploads past their life go, with their spool files.
    pub fn remote_sweep_uploads(&self) {
        let mut rt = self.remote_access.lock().unwrap();
        let gone = rt.uploads.expire(now_ms());
        if !gone.is_empty() {
            tracing::info!("remote access: {} unsent upload(s) expired", gone.len());
        }
        remove_spools(rt.dir(), gone);
        rt.sent.expire(now_ms());
    }

    /// Tests: how many uploads wait, and the spool folder.
    #[cfg(test)]
    pub(crate) fn remote_uploads_for_tests(&self) -> (usize, std::path::PathBuf) {
        let rt = self.remote_access.lock().unwrap();
        (rt.uploads.len(), rt.dir().join(SPOOL_DIR))
    }

    /// Tests: how many images are kept, and the folder they sit in.
    #[cfg(test)]
    pub(crate) fn remote_sent_for_tests(&self) -> (usize, std::path::PathBuf) {
        let rt = self.remote_access.lock().unwrap();
        (rt.sent.len(), rt.sent.dir().to_path_buf())
    }

    /// Tests: every kept image `by_ms` older.
    #[cfg(test)]
    pub(crate) fn remote_sent_age_for_tests(&self, by_ms: u64) {
        self.remote_access.lock().unwrap().sent.age_all(by_ms);
    }

    /// Tests: every waiting upload `by_ms` older; a shorter chunk gap.
    #[cfg(test)]
    pub(crate) fn remote_uploads_tune_for_tests(&self, age_ms: u64, idle: Option<Duration>) {
        let mut rt = self.remote_access.lock().unwrap();
        rt.uploads.age_all(age_ms);
        if let Some(i) = idle {
            rt.upload_idle = i;
        }
    }
}

/// Copy `body` into `file`, the cap counted on every chunk (whatever the
/// request declared), the kind judged on the first bytes, no gap over
/// [`CHUNK_IDLE`]. The size.
async fn read_into<S, E>(file: &mut tokio::fs::File, mut body: S, idle: Duration) -> Result<u64, Refusal>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    use tokio::io::AsyncWriteExt;
    let mut size: u64 = 0;
    let mut head: Vec<u8> = Vec::with_capacity(HEAD);
    let mut judged = false;
    let store = |_| Refusal::Failed("the Mac could not store the file".into());
    loop {
        let next = match tokio::time::timeout(idle, body.next()).await {
            Ok(n) => n,
            Err(_) => return Err(reason(Reason::Timeout)),
        };
        let chunk = match next {
            None => break,
            Some(Ok(c)) => c,
            Some(Err(_)) => return Err(Refusal::Bad("the upload was cut off".into())),
        };
        size += chunk.len() as u64;
        if size > FILE_MAX {
            return Err(reason(Reason::TooLarge));
        }
        if !judged {
            let take = (HEAD - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..take]);
            if head.len() >= HEAD {
                judge(&head)?;
                judged = true;
            }
        }
        file.write_all(&chunk).await.map_err(store)?;
    }
    if size == 0 {
        return Err(reason(Reason::Empty));
    }
    if !judged {
        judge(&head)?;
    }
    file.flush().await.map_err(store)?;
    file.sync_all().await.map_err(store)?;
    Ok(size)
}

/// The Kanban's check (`tickets::sniff`): programs and archives refused.
fn judge(head: &[u8]) -> Result<(), Refusal> {
    match sbxw_core::tickets::sniff(head) {
        Ok(_) => Ok(()),
        Err(_) if head.is_empty() => Err(reason(Reason::Empty)),
        Err(_) => Err(reason(Reason::FileKind)),
    }
}
