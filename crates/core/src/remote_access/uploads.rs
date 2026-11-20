//! Files a paired phone handed to a leader and not yet sent (API.md 2.6.1).
//! Pure rules, time passed in: the daemon streams the bytes to a spool file
//! on the Mac and keeps the handle here; a message that names the handle's
//! id copies the file into the sandbox and spends it.
//!
//! An id is 128 bits of OS randomness, bound to one device and one leader.
//! The book never holds a sandbox path the phone could learn: `copied` is
//! the daemon's own, and no answer serialises an [`Upload`].

use crate::ids::{random_hex, same_secret};

/// One file, at most (API.md 2.6.1: a phone photo or a PDF).
pub const FILE_MAX: u64 = 25 << 20;
/// Files waiting per device and leader (and ids per message).
pub const WAITING_MAX: usize = 5;
/// Bytes waiting per device and leader.
pub const PAIR_TOTAL_MAX: u64 = 60 << 20;
/// Bytes waiting for every device together (the Mac's spool).
pub const ALL_TOTAL_MAX: u64 = 200 << 20;
/// An unsent file lives this long after its upload.
pub const TTL_MS: u64 = 30 * 60 * 1000;
/// Uploads per device per [`RATE_WINDOW_MS`] (refused ones count).
pub const RATE_PER_DEVICE: u32 = 20;
pub const RATE_WINDOW_MS: u64 = 10 * 60 * 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upload {
    pub id: String,
    pub device_id: String,
    pub sandbox: String,
    /// The daemon's name ([`crate::uploads::upload_name`]).
    pub name: String,
    pub size: u64,
    pub expires_ms: u64,
    /// The spool file's name, inside the daemon's spool folder.
    pub spool: String,
    /// Where it was copied into the sandbox, once a message tried to send it.
    pub copied: Option<String>,
}

/// Why there is no room for one more file.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Full {
    /// Already [`WAITING_MAX`] waiting, or a byte cap would be passed.
    TooMany,
}

/// Why a message's ids do not resolve.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Unresolved {
    /// More than [`WAITING_MAX`] ids.
    TooMany,
    /// An id that is not this device's and this leader's, spent, expired,
    /// unknown, or given twice: one answer for all.
    Bad,
}

/// What the journal says of a file's size: a class, never the size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeClass {
    #[serde(rename = "under_1mb")]
    Under1Mb,
    #[serde(rename = "1_to_10mb")]
    OneTo10Mb,
    #[serde(rename = "over_10mb")]
    Over10Mb,
}

impl SizeClass {
    pub fn of(size: u64) -> Self {
        if size < 1 << 20 {
            SizeClass::Under1Mb
        } else if size <= 10 << 20 {
            SizeClass::OneTo10Mb
        } else {
            SizeClass::Over10Mb
        }
    }
}

/// A fresh id: 32 hex, 128 bits.
pub fn new_id() -> String {
    random_hex(16)
}

#[derive(Debug, Default)]
pub struct UploadBook {
    items: Vec<Upload>,
}

impl UploadBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether one more file of `size` bytes fits for this device and leader.
    pub fn room(&self, device_id: &str, sandbox: &str, size: u64) -> Result<(), Full> {
        let mine: Vec<&Upload> = self.items.iter().filter(|u| u.device_id == device_id && u.sandbox == sandbox).collect();
        let pair: u64 = mine.iter().map(|u| u.size).sum();
        let all: u64 = self.items.iter().map(|u| u.size).sum();
        if mine.len() >= WAITING_MAX || pair + size > PAIR_TOTAL_MAX || all + size > ALL_TOTAL_MAX {
            return Err(Full::TooMany);
        }
        Ok(())
    }

    /// Keep a finished upload; [`Self::room`] is checked again here.
    pub fn add(&mut self, u: Upload) -> Result<(), Full> {
        self.room(&u.device_id, &u.sandbox, u.size)?;
        self.items.push(u);
        Ok(())
    }

    fn find(&self, id: &str) -> Option<usize> {
        // Constant time per comparison, no early exit: an id is a secret.
        let mut hit = None;
        for (i, u) in self.items.iter().enumerate() {
            if same_secret(&u.id, id) && hit.is_none() {
                hit = Some(i);
            }
        }
        hit
    }

    /// The uploads a message names, in its order: every id this device's
    /// and this leader's, live, and given once.
    pub fn resolve(&self, device_id: &str, sandbox: &str, ids: &[String], now: u64) -> Result<Vec<Upload>, Unresolved> {
        if ids.len() > WAITING_MAX {
            return Err(Unresolved::TooMany);
        }
        let mut out: Vec<Upload> = Vec::with_capacity(ids.len());
        for id in ids {
            let u = self.find(id).map(|i| &self.items[i]).ok_or(Unresolved::Bad)?;
            if u.device_id != device_id || u.sandbox != sandbox || now >= u.expires_ms || out.iter().any(|o| o.id == u.id) {
                return Err(Unresolved::Bad);
            }
            out.push(u.clone());
        }
        Ok(out)
    }

    /// The file was copied into the sandbox at `path` (a retry reuses it).
    pub fn set_copied(&mut self, id: &str, path: &str) {
        if let Some(i) = self.find(id) {
            self.items[i].copied = Some(path.to_string());
        }
    }

    /// The message was submitted: the handles go.
    pub fn spend(&mut self, ids: &[String]) -> Vec<Upload> {
        let mut out = vec![];
        for id in ids {
            if let Some(i) = self.find(id) {
                out.push(self.items.remove(i));
            }
        }
        out
    }

    /// `DELETE`: this device's file for this leader, not yet sent.
    pub fn drop_one(&mut self, device_id: &str, sandbox: &str, id: &str, now: u64) -> Option<Upload> {
        let i = self.find(id)?;
        let u = &self.items[i];
        if u.device_id != device_id || u.sandbox != sandbox || now >= u.expires_ms {
            return None;
        }
        Some(self.items.remove(i))
    }

    /// A device revoked or logged out: all its files.
    pub fn drop_device(&mut self, device_id: &str) -> Vec<Upload> {
        let (gone, keep) = std::mem::take(&mut self.items).into_iter().partition(|u| u.device_id == device_id);
        self.items = keep;
        gone
    }

    /// Remote access off: everything.
    pub fn clear(&mut self) -> Vec<Upload> {
        std::mem::take(&mut self.items)
    }

    /// Unsent files past their life.
    pub fn expire(&mut self, now: u64) -> Vec<Upload> {
        let (gone, keep) = std::mem::take(&mut self.items).into_iter().partition(|u| now >= u.expires_ms);
        self.items = keep;
        gone
    }

    /// Tests of the daemon: make every handle `by_ms` older.
    #[doc(hidden)]
    pub fn age_all(&mut self, by_ms: u64) {
        for u in &mut self.items {
            u.expires_ms = u.expires_ms.saturating_sub(by_ms);
        }
    }

    /// Every spool file the book still owns (the rest of the folder is not).
    pub fn spools(&self) -> Vec<String> {
        self.items.iter().map(|u| u.spool.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(id: &str, dev: &str, sb: &str, size: u64, now: u64) -> Upload {
        Upload { id: id.into(), device_id: dev.into(), sandbox: sb.into(), name: "a.png".into(), size, expires_ms: now + TTL_MS, spool: format!("{id}.bin"), copied: None }
    }

    #[test]
    fn ids_are_128_bits() {
        let a = new_id();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, new_id());
    }

    #[test]
    fn caps_per_device_leader_and_in_all() {
        let mut b = UploadBook::new();
        for i in 0..WAITING_MAX {
            b.add(up(&format!("{i}"), "d1", "lead", 1, 0)).unwrap();
        }
        assert_eq!(b.room("d1", "lead", 1), Err(Full::TooMany), "five waiting");
        assert!(b.room("d1", "other", 1).is_ok(), "another leader has its own five");
        assert!(b.room("d2", "lead", 1).is_ok(), "another device too");
        let mut b = UploadBook::new();
        b.add(up("x", "d1", "lead", FILE_MAX, 0)).unwrap();
        b.add(up("y", "d1", "lead", FILE_MAX, 0)).unwrap();
        assert_eq!(b.room("d1", "lead", PAIR_TOTAL_MAX - 2 * FILE_MAX + 1), Err(Full::TooMany), "60 MiB per device and leader");
        let mut b = UploadBook::new();
        for d in 0..8 {
            b.add(up(&format!("i{d}"), &format!("d{d}"), "lead", FILE_MAX, 0)).unwrap();
        }
        assert_eq!(b.room("d9", "lead", 1), Err(Full::TooMany), "200 MiB in all");
    }

    #[test]
    fn an_id_resolves_only_for_its_device_and_leader_while_alive() {
        let mut b = UploadBook::new();
        b.add(up("aaaa", "d1", "lead", 10, 0)).unwrap();
        b.add(up("bbbb", "d1", "lead", 10, 0)).unwrap();
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(b.resolve("d1", "lead", &ids(&["bbbb", "aaaa"]), 1).unwrap().iter().map(|u| u.id.as_str()).collect::<Vec<_>>(), ["bbbb", "aaaa"]);
        assert_eq!(b.resolve("d2", "lead", &ids(&["aaaa"]), 1), Err(Unresolved::Bad), "another device");
        assert_eq!(b.resolve("d1", "other", &ids(&["aaaa"]), 1), Err(Unresolved::Bad), "another leader");
        assert_eq!(b.resolve("d1", "lead", &ids(&["aaaa", "aaaa"]), 1), Err(Unresolved::Bad), "twice");
        assert_eq!(b.resolve("d1", "lead", &ids(&["cccc"]), 1), Err(Unresolved::Bad), "forged");
        assert_eq!(b.resolve("d1", "lead", &ids(&["aaaa"]), TTL_MS), Err(Unresolved::Bad), "expired");
        assert_eq!(b.resolve("d1", "lead", &ids(&["1", "2", "3", "4", "5", "6"]), 1), Err(Unresolved::TooMany));
        b.set_copied("aaaa", "/tmp/sbxw-uploads/1/a.png");
        assert_eq!(b.resolve("d1", "lead", &ids(&["aaaa"]), 1).unwrap()[0].copied.as_deref(), Some("/tmp/sbxw-uploads/1/a.png"));
        assert_eq!(b.spend(&ids(&["aaaa"])).len(), 1);
        assert_eq!(b.resolve("d1", "lead", &ids(&["aaaa"]), 1), Err(Unresolved::Bad), "spent");
        assert!(b.drop_one("d2", "lead", "bbbb", 1).is_none(), "another device cannot drop it");
        assert!(b.drop_one("d1", "lead", "bbbb", 1).is_some());
        assert!(b.is_empty());
    }

    #[test]
    fn expiry_revoke_and_clear() {
        let mut b = UploadBook::new();
        b.add(up("a", "d1", "lead", 1, 0)).unwrap();
        b.add(up("b", "d2", "lead", 1, 0)).unwrap();
        b.add(up("c", "d2", "lead", 1, 1000)).unwrap();
        assert!(b.expire(TTL_MS - 1).is_empty());
        assert_eq!(b.expire(TTL_MS).len(), 2);
        assert_eq!(b.spools(), ["c.bin"]);
        assert_eq!(b.drop_device("d2").len(), 1);
        b.add(up("d", "d1", "lead", 1, 0)).unwrap();
        assert_eq!(b.clear().len(), 1);
        assert!(b.is_empty());
    }

    #[test]
    fn size_classes() {
        assert_eq!(SizeClass::of(1), SizeClass::Under1Mb);
        assert_eq!(SizeClass::of(1 << 20), SizeClass::OneTo10Mb);
        assert_eq!(SizeClass::of(11 << 20), SizeClass::Over10Mb);
        assert_eq!(serde_json::to_string(&SizeClass::OneTo10Mb).unwrap(), "\"1_to_10mb\"");
    }
}
