//! The UI token: proof that a request to the UI listener comes from this user.
//!
//! Minted at every daemon start (256 bits), written to `<sbxw home>/run/ui-token`
//! (file `0600`, directory `0700`), removed at shutdown. Native clients read the
//! file and send `Authorization: Bearer`; a browser gets it once through the
//! sign-in address `sbxw web` opens and keeps it as an `HttpOnly` cookie.
//!
//! Before a client sends the token (or any secret) it asks `/api/health?nonce=`
//! for [`proof`]: only a daemon that knows the token can answer it, so a process
//! squatting the port learns nothing and is refused.

use crate::store::Paths;
use std::io::Write;
use std::path::PathBuf;

/// The cookie a signed-in browser carries.
pub const COOKIE: &str = "sbxw_ui";

/// Where the running daemon's token is.
pub fn path(paths: &Paths) -> PathBuf {
    paths.run().join("ui-token")
}

/// A fresh token: 256 bits.
pub fn mint() -> String {
    format!("sbxw_ui_{}", sbxw_core::ids::random_hex(32))
}

/// The token file's contents, when there is one.
pub fn read(paths: &Paths) -> Option<String> {
    let t = std::fs::read_to_string(path(paths)).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Write `token`, replacing an earlier daemon's. The run directory is made
/// `0700`; the file is created `0600` under another name and renamed, so no
/// reader sees it half written and nothing is followed through a planted link.
pub fn write(paths: &Paths, token: &str) -> std::io::Result<()> {
    let dir = paths.run();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!("ui-token.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        open.mode(0o600);
    }
    let mut f = open.open(&tmp)?;
    f.write_all(token.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path(paths))
}

/// Remove the file, if it still holds `token` (a newer daemon's is left alone).
pub fn remove(paths: &Paths, token: &str) {
    if read(paths).is_some_and(|t| sbxw_core::ids::same_secret(&t, token)) {
        let _ = std::fs::remove_file(path(paths));
    }
}

/// SHA-256(token || nonce), lower-case hex: what `/api/health?nonce=` answers.
pub fn proof(token: &str, nonce: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    h.update(nonce.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_is_private_replaced_and_removed_only_when_ours() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::under(d.path());
        let (a, b) = (mint(), mint());
        assert_ne!(a, b);
        assert_eq!(a.len(), 8 + 64);
        write(&paths, &a).unwrap();
        write(&paths, &b).unwrap();
        assert_eq!(read(&paths).as_deref(), Some(b.as_str()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path(&paths)), 0o600);
            assert_eq!(mode(&paths.run()), 0o700);
        }
        remove(&paths, &a);
        assert!(read(&paths).is_some(), "another daemon's token stays");
        remove(&paths, &b);
        assert!(read(&paths).is_none());
    }

    #[test]
    fn the_proof_is_sha256_of_token_then_nonce() {
        // echo -n "abc" | sha256sum
        assert_eq!(proof("a", "bc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
