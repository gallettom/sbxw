//! Paired devices. A device is its token's SHA-256, a public id, the name
//! the phone gave and two times. The token itself is never kept: it lives in
//! the phone's cookie only.

use super::{hash_token, DEVICE_IDLE_MS};
use serde::{Deserialize, Serialize};

/// `last_seen_ms` is written at most this often per device.
pub const TOUCH_EVERY_MS: u64 = 60_000;

/// Devices kept at most; pairing past it is refused.
pub const MAX_DEVICES: usize = 32;

/// A device as stored on disk.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub token_sha256: String,
    pub created_ms: u64,
    pub last_seen_ms: u64,
}

/// A device as any route shows it: never the hash.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub created_ms: u64,
    pub last_seen_ms: u64,
}

impl Device {
    pub fn view(&self) -> DeviceView {
        DeviceView { id: self.id.clone(), name: self.name.clone(), created_ms: self.created_ms, last_seen_ms: self.last_seen_ms }
    }
}

/// A fresh device token: 256 bits.
pub fn mint_token() -> String {
    format!("sbxw_dev_{}", crate::ids::random_hex(32))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceBook {
    devices: Vec<Device>,
}

impl DeviceBook {
    pub fn from_devices(mut devices: Vec<Device>) -> Self {
        // A record that could not have come from `mint` is dropped.
        devices.retain(|d| d.token_sha256.len() == 64 && d.token_sha256.bytes().all(|b| b.is_ascii_hexdigit()) && !d.id.is_empty());
        DeviceBook { devices }
    }

    pub fn devices(&self) -> &[Device] {
        &self.devices
    }

    pub fn views(&self) -> Vec<DeviceView> {
        self.devices.iter().map(Device::view).collect()
    }

    pub fn len(&self) -> usize {
        self.devices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// Pair a device: returns it and its token, the one time the token
    /// exists outside a phone. `None` past [`MAX_DEVICES`].
    pub fn mint(&mut self, name: &str, now: u64) -> Option<(Device, String)> {
        if self.devices.len() >= MAX_DEVICES {
            return None;
        }
        let token = mint_token();
        let d = Device { id: crate::ids::random_hex(8), name: name.to_string(), token_sha256: hash_token(&token), created_ms: now, last_seen_ms: now };
        self.devices.push(d.clone());
        Some((d, token))
    }

    /// The device `token` belongs to. Every stored hash is compared in
    /// constant time, with no early exit, so the time taken says nothing
    /// about which or how many matched. An idle device does not match.
    pub fn find(&self, token: &str, now: u64) -> Option<&Device> {
        if token.is_empty() || token.len() > 256 {
            return None;
        }
        let h = hash_token(token);
        let mut found: Option<&Device> = None;
        for d in &self.devices {
            let same = crate::ids::same_secret(&h, &d.token_sha256);
            if same && found.is_none() {
                found = Some(d);
            }
        }
        found.filter(|d| now.saturating_sub(d.last_seen_ms) <= DEVICE_IDLE_MS)
    }

    /// Note that `id` was seen; true when the record changed (worth a write).
    pub fn touch(&mut self, id: &str, now: u64) -> bool {
        match self.devices.iter_mut().find(|d| d.id == id) {
            Some(d) if now.saturating_sub(d.last_seen_ms) >= TOUCH_EVERY_MS => {
                d.last_seen_ms = now;
                true
            }
            _ => false,
        }
    }

    pub fn get(&self, id: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn revoke(&mut self, id: &str) -> Option<Device> {
        let i = self.devices.iter().position(|d| d.id == id)?;
        Some(self.devices.remove(i))
    }

    /// Drop every device idle past [`DEVICE_IDLE_MS`]; returns them.
    pub fn expire(&mut self, now: u64) -> Vec<Device> {
        let (gone, kept): (Vec<Device>, Vec<Device>) = std::mem::take(&mut self.devices).into_iter().partition(|d| now.saturating_sub(d.last_seen_ms) > DEVICE_IDLE_MS);
        self.devices = kept;
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_kept_as_hashes_only() {
        let mut b = DeviceBook::default();
        let (d, token) = b.mint("phone", 1000).unwrap();
        assert!(token.starts_with("sbxw_dev_") && token.len() == 9 + 64);
        assert_ne!(d.token_sha256, token);
        let json = serde_json::to_string(b.devices()).unwrap();
        assert!(!json.contains(&token), "the token is never stored");
        assert!(!json.contains(&token["sbxw_dev_".len()..]), "not even its random part");
        assert!(!serde_json::to_string(&b.views()).unwrap().contains(&d.token_sha256), "views carry no hash");
        assert_eq!(b.find(&token, 1000).map(|d| d.id.clone()), Some(d.id.clone()));
        assert!(b.find("sbxw_dev_nope", 1000).is_none());
        assert!(b.find(&d.token_sha256, 1000).is_none(), "the hash is not a token");
        assert!(b.find("", 1000).is_none());
    }

    #[test]
    fn idle_devices_expire_and_touch_is_throttled() {
        let mut b = DeviceBook::default();
        let (d, token) = b.mint("phone", 0).unwrap();
        assert!(!b.touch(&d.id, TOUCH_EVERY_MS - 1));
        assert!(b.touch(&d.id, TOUCH_EVERY_MS));
        let seen = TOUCH_EVERY_MS;
        assert!(b.find(&token, seen + DEVICE_IDLE_MS).is_some());
        assert!(b.find(&token, seen + DEVICE_IDLE_MS + 1).is_none(), "30 days idle: no longer a key");
        assert!(b.expire(seen + DEVICE_IDLE_MS).is_empty());
        assert_eq!(b.expire(seen + DEVICE_IDLE_MS + 1).len(), 1);
        assert!(b.is_empty());
    }

    #[test]
    fn revoke_and_cap() {
        let mut b = DeviceBook::default();
        let (d, token) = b.mint("a", 0).unwrap();
        assert!(b.revoke(&d.id).is_some());
        assert!(b.find(&token, 0).is_none());
        assert!(b.revoke(&d.id).is_none());
        for i in 0..MAX_DEVICES {
            assert!(b.mint(&format!("d{i}"), 0).is_some());
        }
        assert!(b.mint("one more", 0).is_none());
    }

    #[test]
    fn a_tampered_record_is_dropped() {
        let ok = Device { id: "ab".into(), name: "x".into(), token_sha256: "0".repeat(64), created_ms: 0, last_seen_ms: 0 };
        let bad = Device { token_sha256: "plaintext-token".into(), ..ok.clone() };
        assert_eq!(DeviceBook::from_devices(vec![ok, bad]).len(), 1);
    }
}
