//! The images a paired phone sent, kept on the Mac so its conversation can
//! show them again (`docs/security.md#remote-access`). A copy is taken when
//! the message goes (the upload is spent); it lives [`KEEP_MS`], at most
//! [`KEEP_BYTES_MAX`] in all, the oldest removed first.
//!
//! Folder `<sbxw home>/remote/sent/` is `0700`, files `0600`, named by a
//! random id; no name from outside is ever a path. `index.json` beside them
//! says, per image, the device that sent it, the sandbox, and where the file
//! sat in that sandbox (`/tmp/sbxw-uploads/<ms>/<name>`): the history's own
//! block names exactly that, so `(sandbox, ms, name)` finds the image of a
//! file shown in a message with no guessing, and the history needs no id.
//!
//! The kind is judged on the bytes (PNG, JPEG, WebP); the name never decides.
//! Never logged: a name, a path, a content.

use super::remote_access::write_private;
use sbxw_core::ids::{random_hex, same_secret};
use sbxw_core::tickets::FileKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The folder, under `<sbxw home>/remote/`.
pub const SENT_DIR: &str = "sent";
/// A kept image lives this long after it was sent.
pub const KEEP_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// Bytes kept for every device together.
pub const KEEP_BYTES_MAX: u64 = 500 << 20;
const INDEX: &str = "index.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sent {
    pub id: String,
    pub device_id: String,
    pub sandbox: String,
    /// The upload folder's name in the sandbox, and the file's name there.
    pub ms: u64,
    pub name: String,
    pub kind: FileKind,
    pub size: u64,
    /// When it was kept.
    pub at: u64,
}

impl Sent {
    fn file(&self) -> String {
        format!("{}.{}", self.id, self.kind.ext())
    }
}

/// `(ms, name)` of `/tmp/sbxw-uploads/<ms>/<name>`.
pub fn split_dest(path: &str) -> Option<(u64, &str)> {
    let rest = path.strip_prefix(sbxw_core::uploads::UPLOAD_ROOT)?.strip_prefix('/')?;
    let (ms, name) = rest.split_once('/')?;
    if name.is_empty() || name.contains('/') {
        return None;
    }
    Some((ms.parse().ok()?, name))
}

/// A picture kind the phone may be shown again, judged on the first bytes.
pub fn picture_kind(head: &[u8]) -> Option<FileKind> {
    match sbxw_core::tickets::sniff(head) {
        Ok(k @ (FileKind::Png | FileKind::Jpeg | FileKind::Webp)) => Some(k),
        _ => None,
    }
}

#[derive(Debug, Default)]
pub struct SentStore {
    dir: PathBuf,
    items: Vec<Sent>,
}

impl SentStore {
    /// Read the index; entries past their life, and files the index does not
    /// name, go.
    pub fn open(remote_dir: &Path, now: u64) -> Self {
        let dir = remote_dir.join(SENT_DIR);
        let items: Vec<Sent> = std::fs::read(dir.join(INDEX)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut s = SentStore { dir, items };
        s.expire(now);
        if let Ok(rd) = std::fs::read_dir(&s.dir) {
            let known: Vec<String> = s.items.iter().map(Sent::file).collect();
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n != INDEX && !known.contains(&n) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        s
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn total(&self) -> u64 {
        self.items.iter().map(|s| s.size).sum()
    }

    fn save(&self) {
        if let Ok(b) = serde_json::to_vec(&self.items) {
            if write_private(&self.dir, &self.dir.join(INDEX), &b).is_err() {
                tracing::warn!("remote access: the kept images' index could not be written");
            }
        }
    }

    fn remove_files(&self, gone: &[Sent]) {
        for s in gone {
            let _ = std::fs::remove_file(self.dir.join(s.file()));
        }
    }

    /// Copy `src` (a spool file) into the store under a random name, `0600`.
    /// Not yet in the index: [`Self::add`] does that. `None` when it is not
    /// a kept kind, too large for the cap, or cannot be copied.
    pub fn copy_in(dir: &Path, src: &Path, device_id: &str, sandbox: &str, dest: &str, now: u64) -> Option<Sent> {
        use std::io::Read;
        let (ms, name) = split_dest(dest)?;
        let mut f = std::fs::File::open(src).ok()?;
        let size = f.metadata().ok()?.len();
        if size == 0 || size > KEEP_BYTES_MAX {
            return None;
        }
        let mut head = vec![0u8; 512];
        let n = f.read(&mut head).ok()?;
        let kind = picture_kind(&head[..n])?;
        drop(f);
        let store = dir.join(SENT_DIR);
        std::fs::create_dir_all(&store).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).ok()?;
        }
        let s = Sent { id: random_hex(16), device_id: device_id.into(), sandbox: sandbox.into(), ms, name: name.into(), kind, size, at: now };
        let to = store.join(s.file());
        let mut open = std::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        let mut out = open.open(&to).ok()?;
        let copied = std::fs::File::open(src).and_then(|mut i| std::io::copy(&mut i, &mut out));
        if copied.is_err() || out.sync_all().is_err() {
            let _ = std::fs::remove_file(&to);
            return None;
        }
        Some(s)
    }

    /// Keep a copied image. The oldest go while the cap is passed. The same
    /// file sent twice (same sandbox, folder and name) replaces the earlier.
    pub fn add(&mut self, s: Sent) {
        let (older, keep): (Vec<Sent>, Vec<Sent>) = std::mem::take(&mut self.items).into_iter().partition(|o| o.sandbox == s.sandbox && o.ms == s.ms && o.name == s.name);
        self.items = keep;
        self.remove_files(&older);
        self.items.push(s);
        let mut gone = vec![];
        while self.total() > KEEP_BYTES_MAX && self.items.len() > 1 {
            // Oldest first: by when it was kept.
            let i = (0..self.items.len()).min_by_key(|&i| self.items[i].at).unwrap_or(0);
            gone.push(self.items.remove(i));
        }
        self.remove_files(&gone);
        self.save();
    }

    /// Past their life.
    pub fn expire(&mut self, now: u64) -> usize {
        let (gone, keep): (Vec<Sent>, Vec<Sent>) = std::mem::take(&mut self.items).into_iter().partition(|s| now >= s.at.saturating_add(KEEP_MS));
        self.items = keep;
        self.remove_files(&gone);
        if !gone.is_empty() {
            self.save();
        }
        gone.len()
    }

    /// A device revoked or logged out: its images.
    pub fn drop_device(&mut self, device_id: &str) {
        let (gone, keep): (Vec<Sent>, Vec<Sent>) = std::mem::take(&mut self.items).into_iter().partition(|s| s.device_id == device_id);
        self.items = keep;
        self.remove_files(&gone);
        if !gone.is_empty() {
            self.save();
        }
    }

    /// Remote access off: everything.
    pub fn clear(&mut self) {
        let gone = std::mem::take(&mut self.items);
        self.remove_files(&gone);
        if !gone.is_empty() {
            self.save();
        }
    }

    /// Tests: every image kept `by_ms` earlier.
    #[cfg(test)]
    pub fn age_all(&mut self, by_ms: u64) {
        for s in &mut self.items {
            s.at = s.at.saturating_sub(by_ms);
        }
    }

    /// The id of the image this device sent as `(sandbox, ms, name)`, if
    /// still kept.
    pub fn find(&self, device_id: &str, sandbox: &str, ms: u64, name: &str, now: u64) -> Option<String> {
        self.items.iter().find(|s| s.device_id == device_id && s.sandbox == sandbox && s.ms == ms && s.name == name && now < s.at.saturating_add(KEEP_MS)).map(|s| s.id.clone())
    }

    /// One image for `GET /r/sent/:id`: this device's, live. The sandbox,
    /// the file and its content type.
    pub fn get(&self, device_id: &str, id: &str, now: u64) -> Option<(String, PathBuf, &'static str)> {
        // Constant time per comparison, no early exit: an id is a secret.
        let mut hit = None;
        for s in &self.items {
            if same_secret(&s.id, id) && hit.is_none() {
                hit = Some(s);
            }
        }
        let s = hit?;
        if s.device_id != device_id || now >= s.at.saturating_add(KEEP_MS) {
            return None;
        }
        Some((s.sandbox.clone(), self.dir.join(s.file()), s.kind.mime()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n0000";

    fn spool(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn keep(store: &mut SentStore, dir: &Path, src: &Path, dev: &str, dest: &str, now: u64) -> Option<String> {
        let s = SentStore::copy_in(dir, src, dev, "lead", dest, now)?;
        let id = s.id.clone();
        store.add(s);
        Some(id)
    }

    #[test]
    fn dest_is_split_and_nothing_else() {
        assert_eq!(split_dest("/tmp/sbxw-uploads/17/a.png"), Some((17, "a.png")));
        assert_eq!(split_dest("/tmp/sbxw-uploads/17/../a.png"), None);
        assert_eq!(split_dest("/tmp/sbxw-uploads/x/a.png"), None);
        assert_eq!(split_dest("/etc/passwd"), None);
    }

    #[test]
    fn only_pictures_are_kept_judged_on_the_bytes() {
        let t = tempfile::tempdir().unwrap();
        let mut st = SentStore::open(t.path(), 0);
        let png = spool(t.path(), "a.bin", PNG);
        let txt = spool(t.path(), "b.bin", b"hello");
        let pdf = spool(t.path(), "c.bin", b"%PDF-1.7 ....");
        let gif = spool(t.path(), "d.bin", b"GIF89a....");
        assert!(keep(&mut st, t.path(), &png, "d1", "/tmp/sbxw-uploads/1/not-a-name.txt", 10).is_some(), "the name never decides");
        for (f, n) in [(&txt, "t.png"), (&pdf, "p.png"), (&gif, "g.png")] {
            assert!(keep(&mut st, t.path(), f, "d1", &format!("/tmp/sbxw-uploads/2/{n}"), 10).is_none());
        }
        assert!(keep(&mut st, t.path(), &png, "d1", "/etc/x", 10).is_none(), "a dest outside the upload folder");
        assert_eq!(st.len(), 1);
        assert_eq!(std::fs::read_dir(st.dir()).unwrap().count(), 2, "the image and the index");
    }

    #[test]
    fn folder_and_files_are_private_with_generated_names() {
        let t = tempfile::tempdir().unwrap();
        let mut st = SentStore::open(t.path(), 0);
        let png = spool(t.path(), "a.bin", PNG);
        let id = keep(&mut st, t.path(), &png, "d1", "/tmp/sbxw-uploads/1/IMG 1.png", 10).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(st.dir()), 0o700);
            assert_eq!(mode(&st.dir().join(format!("{id}.png"))), 0o600);
            assert_eq!(mode(&st.dir().join(INDEX)), 0o600);
        }
        let names: Vec<String> = std::fs::read_dir(st.dir()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        assert!(names.iter().all(|n| !n.contains("IMG")), "{names:?}");
    }

    #[test]
    fn found_by_sandbox_folder_and_name_for_that_device_only() {
        let t = tempfile::tempdir().unwrap();
        let mut st = SentStore::open(t.path(), 0);
        let png = spool(t.path(), "a.bin", PNG);
        let a = keep(&mut st, t.path(), &png, "d1", "/tmp/sbxw-uploads/1/image.png", 10).unwrap();
        let b = keep(&mut st, t.path(), &png, "d1", "/tmp/sbxw-uploads/2/image.png", 10).unwrap();
        assert_eq!(st.find("d1", "lead", 1, "image.png", 11), Some(a.clone()));
        assert_eq!(st.find("d1", "lead", 2, "image.png", 11), Some(b), "the same name sent twice is two images");
        assert_eq!(st.find("d2", "lead", 1, "image.png", 11), None);
        assert_eq!(st.find("d1", "other", 1, "image.png", 11), None);
        assert!(st.get("d1", &a, 11).is_some());
        assert!(st.get("d2", &a, 11).is_none(), "another device's id");
        assert!(st.get("d1", &"0".repeat(32), 11).is_none(), "unknown id");
        let (sb, path, mime) = st.get("d1", &a, 11).unwrap();
        assert_eq!((sb.as_str(), mime), ("lead", "image/png"));
        assert_eq!(std::fs::read(path).unwrap(), PNG);
    }

    #[test]
    fn expiry_after_seven_days_removes_the_file() {
        let t = tempfile::tempdir().unwrap();
        let mut st = SentStore::open(t.path(), 0);
        let png = spool(t.path(), "a.bin", PNG);
        let id = keep(&mut st, t.path(), &png, "d1", "/tmp/sbxw-uploads/1/a.png", 1000).unwrap();
        assert_eq!(KEEP_MS, 7 * 86_400_000);
        assert!(st.get("d1", &id, 1000 + KEEP_MS - 1).is_some());
        assert!(st.get("d1", &id, 1000 + KEEP_MS).is_none(), "expired, even before a sweep");
        assert_eq!(st.find("d1", "lead", 1, "a.png", 1000 + KEEP_MS), None);
        assert_eq!(st.expire(1000 + KEEP_MS - 1), 0);
        assert_eq!(st.expire(1000 + KEEP_MS), 1);
        assert!(st.is_empty());
        assert!(!st.dir().join(format!("{id}.png")).exists());
    }

    #[test]
    fn the_cap_removes_the_oldest_first() {
        let t = tempfile::tempdir().unwrap();
        let mut st = SentStore::open(t.path(), 0);
        let png = spool(t.path(), "a.bin", PNG);
        let mut ids = vec![];
        for i in 0..3u64 {
            let s = SentStore::copy_in(t.path(), &png, "d1", "lead", &format!("/tmp/sbxw-uploads/{i}/a.png"), 100 + i).unwrap();
            ids.push(s.id.clone());
            // Pretend each is 200 MB: the third passes the 500 MB cap.
            st.add(Sent { size: 200 << 20, ..s });
        }
        assert_eq!(st.len(), 2);
        assert!(st.total() <= KEEP_BYTES_MAX);
        assert!(st.get("d1", &ids[0], 200).is_none(), "the oldest went");
        assert!(!st.dir().join(format!("{}.png", ids[0])).exists());
        assert!(st.get("d1", &ids[2], 200).is_some());
    }

    #[test]
    fn one_file_larger_than_the_cap_is_not_kept() {
        let t = tempfile::tempdir().unwrap();
        let f = std::fs::File::create(t.path().join("big.bin")).unwrap();
        let mut head = PNG.to_vec();
        head.resize(1024, 0);
        std::fs::write(t.path().join("big.bin"), &head).unwrap();
        f.set_len(KEEP_BYTES_MAX + 1).unwrap();
        assert!(SentStore::copy_in(t.path(), &t.path().join("big.bin"), "d1", "lead", "/tmp/sbxw-uploads/1/a.png", 1).is_none());
    }

    #[test]
    fn the_index_survives_a_restart_and_strays_go() {
        let t = tempfile::tempdir().unwrap();
        let mut st = SentStore::open(t.path(), 0);
        let png = spool(t.path(), "a.bin", PNG);
        let id = keep(&mut st, t.path(), &png, "d1", "/tmp/sbxw-uploads/1/a.png", 50).unwrap();
        std::fs::write(st.dir().join("stray.png"), b"x").unwrap();
        let st = SentStore::open(t.path(), 60);
        assert!(st.get("d1", &id, 60).is_some());
        assert!(!st.dir().join("stray.png").exists());
        let st = SentStore::open(t.path(), 50 + KEEP_MS);
        assert!(st.is_empty(), "expired while the daemon was off");
    }

    #[test]
    fn revoke_and_off_remove_images() {
        let t = tempfile::tempdir().unwrap();
        let mut st = SentStore::open(t.path(), 0);
        let png = spool(t.path(), "a.bin", PNG);
        keep(&mut st, t.path(), &png, "d1", "/tmp/sbxw-uploads/1/a.png", 1).unwrap();
        keep(&mut st, t.path(), &png, "d2", "/tmp/sbxw-uploads/2/a.png", 1).unwrap();
        st.drop_device("d1");
        assert_eq!(st.len(), 1);
        st.clear();
        assert!(st.is_empty());
        assert_eq!(std::fs::read_dir(st.dir()).unwrap().count(), 1, "only the index");
    }
}
