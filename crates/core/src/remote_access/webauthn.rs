//! WebAuthn for level 2 (see `.sbxw-artifacts/remote/API-level2.md`): the
//! daemon is the relying party of one origin, `https://<tailnet name>`, and
//! accepts one algorithm, ES256 (ECDSA P-256 with SHA-256).
//!
//! Everything a phone sends is untrusted: base64url is decoded strictly, CBOR
//! by [`super::cbor`], `authenticatorData` by a fixed-layout parser with
//! checked offsets. Each check has its own [`Fail`] so a test can show that
//! removing it lets a forged input through.
//!
//! Signature verification and the curve-point check are `p256`'s (RustCrypto).

#![forbid(unsafe_code)]

use super::cbor::{self, Value};
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

/// Flags of `authenticatorData`.
pub const FLAG_UP: u8 = 0x01;
pub const FLAG_UV: u8 = 0x04;
pub const FLAG_AT: u8 = 0x40;
pub const FLAG_ED: u8 = 0x80;

/// COSE: key type EC2, algorithm ES256, curve P-256.
pub const COSE_KTY_EC2: i128 = 2;
pub const COSE_ALG_ES256: i128 = -7;
pub const COSE_CRV_P256: i128 = 1;

pub const MAX_CLIENT_DATA: usize = 2048;
pub const MAX_ATTESTATION: usize = 4096;
pub const MAX_AUTH_DATA: usize = 1024;
pub const MAX_SIGNATURE: usize = 80;
pub const MIN_CREDENTIAL_ID: usize = 16;
pub const MAX_CREDENTIAL_ID: usize = 1023;

/// Why a ceremony is refused. The phone is told one sentence for all of
/// them; the variants are for tests and debug logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fail {
    /// base64url that is not strict, or over its cap.
    Encoding,
    ClientDataJson,
    Type,
    Challenge,
    Origin,
    CrossOrigin,
    TopOrigin,
    Cbor,
    /// `fmt` is not `none`, or `attStmt` is not empty.
    Attestation,
    AuthData,
    RpIdHash,
    UserPresent,
    UserVerified,
    /// No attested credential in a registration, or one in an assertion.
    AttestedCredential,
    CredentialId,
    /// Not EC2, not ES256, not P-256, a coordinate of the wrong size, or an
    /// extra key in the COSE map.
    KeyShape,
    /// The coordinates are not a point of P-256.
    KeyPoint,
    Signature,
    /// A non-zero counter that is not above the stored one.
    CounterRegression,
}

/// Equal bytes, compared without an early exit.
fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ── base64url (RFC 4648 section 5), strict ───────────────────────────────

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn b64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        let chars = chunk.len() + 1;
        for i in 0..chars {
            out.push(B64[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
        }
    }
    out
}

/// Decode unpadded base64url of at most `max` decoded bytes. Refused: any
/// character outside the alphabet (padding included), a length of 1 mod 4,
/// and unused trailing bits that are not zero (one text per byte string).
pub fn b64url_decode(s: &str, max: usize) -> Result<Vec<u8>, Fail> {
    let b = s.as_bytes();
    if b.len() % 4 == 1 || b.len() / 4 * 3 + (b.len() % 4).saturating_sub(1) > max {
        return Err(Fail::Encoding);
    }
    let val = |c: u8| -> Result<u32, Fail> {
        Ok(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return Err(Fail::Encoding),
        } as u32)
    };
    let mut out = Vec::with_capacity(b.len() / 4 * 3 + 2);
    for chunk in b.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= val(*c)? << (18 - 6 * i);
        }
        let bytes = chunk.len() - 1;
        // The bits past the last whole byte must be zero.
        let unused = 24 - 8 * bytes as u32;
        if chunk.len() < 4 && n & ((1 << unused) - 1) != 0 {
            return Err(Fail::Encoding);
        }
        for i in 0..bytes {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Ok(out)
}

// ── authenticatorData ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthData {
    pub rp_id_hash: [u8; 32],
    pub flags: u8,
    pub counter: u32,
    /// Present when the AT flag is set: the credential id and its key as an
    /// uncompressed SEC1 point (`04 || x || y`), already checked on the curve.
    pub attested: Option<(Vec<u8>, Vec<u8>)>,
}

fn slice(input: &[u8], at: usize, n: usize) -> Result<&[u8], Fail> {
    let end = at.checked_add(n).ok_or(Fail::AuthData)?;
    input.get(at..end).ok_or(Fail::AuthData)
}

/// Parse `authenticatorData`: 32 bytes of RP ID hash, a flags byte, a 4-byte
/// counter; then, if AT, an AAGUID (16), a 2-byte credential id length, the
/// id and a COSE key; then, if ED, one CBOR map of extensions; then nothing.
pub fn parse_auth_data(input: &[u8]) -> Result<AuthData, Fail> {
    if input.len() < 37 || input.len() > MAX_AUTH_DATA + MAX_ATTESTATION {
        return Err(Fail::AuthData);
    }
    let rp_id_hash: [u8; 32] = slice(input, 0, 32)?.try_into().map_err(|_| Fail::AuthData)?;
    let flags = slice(input, 32, 1)?[0];
    let counter = u32::from_be_bytes(slice(input, 33, 4)?.try_into().map_err(|_| Fail::AuthData)?);
    let mut at = 37usize;
    let mut attested = None;
    if flags & FLAG_AT != 0 {
        let _aaguid = slice(input, at, 16)?;
        at = at.checked_add(16).ok_or(Fail::AuthData)?;
        let len = u16::from_be_bytes(slice(input, at, 2)?.try_into().map_err(|_| Fail::AuthData)?) as usize;
        at = at.checked_add(2).ok_or(Fail::AuthData)?;
        if !(MIN_CREDENTIAL_ID..=MAX_CREDENTIAL_ID).contains(&len) {
            return Err(Fail::CredentialId);
        }
        let id = slice(input, at, len)?.to_vec();
        at = at.checked_add(len).ok_or(Fail::AuthData)?;
        let rest = input.get(at..).ok_or(Fail::AuthData)?;
        let (key, used) = cbor::decode_prefix(rest).map_err(|_| Fail::Cbor)?;
        at = at.checked_add(used).ok_or(Fail::AuthData)?;
        attested = Some((id, cose_es256(&key)?));
    }
    if flags & FLAG_ED != 0 {
        let rest = input.get(at..).ok_or(Fail::AuthData)?;
        let (ext, used) = cbor::decode_prefix(rest).map_err(|_| Fail::Cbor)?;
        if !matches!(ext, Value::Map(_)) {
            return Err(Fail::AuthData);
        }
        at = at.checked_add(used).ok_or(Fail::AuthData)?;
    }
    if at != input.len() {
        return Err(Fail::AuthData);
    }
    Ok(AuthData { rp_id_hash, flags, counter, attested })
}

/// A COSE key that is exactly `{1: 2, 3: -7, -1: 1, -2: x, -3: y}` with
/// 32-byte coordinates on P-256: its uncompressed SEC1 point.
pub fn cose_es256(key: &Value) -> Result<Vec<u8>, Fail> {
    let Value::Map(m) = key else { return Err(Fail::KeyShape) };
    let allowed = [1, 3, -1, -2, -3];
    if m.len() != allowed.len() || m.iter().any(|(k, _)| !k.int().is_some_and(|k| allowed.contains(&k))) {
        return Err(Fail::KeyShape);
    }
    let int = |k| key.get_int(k).and_then(Value::int);
    if int(1) != Some(COSE_KTY_EC2) || int(3) != Some(COSE_ALG_ES256) || int(-1) != Some(COSE_CRV_P256) {
        return Err(Fail::KeyShape);
    }
    let coord = |k| match key.get_int(k) {
        Some(Value::Bytes(b)) if b.len() == 32 => Ok(*b),
        _ => Err(Fail::KeyShape),
    };
    let mut sec1 = Vec::with_capacity(65);
    sec1.push(0x04);
    sec1.extend_from_slice(coord(-2)?);
    sec1.extend_from_slice(coord(-3)?);
    VerifyingKey::from_sec1_bytes(&sec1).map_err(|_| Fail::KeyPoint)?;
    Ok(sec1)
}

// ── clientDataJSON ───────────────────────────────────────────────────────

/// `clientDataJSON` as the browser wrote it: the ceremony's type, our
/// challenge, our origin exactly, not cross-origin, no top origin.
pub fn check_client_data(raw: &[u8], want_type: &str, challenge: &[u8], origin: &str) -> Result<(), Fail> {
    if raw.len() > MAX_CLIENT_DATA {
        return Err(Fail::ClientDataJson);
    }
    let v: serde_json::Value = serde_json::from_slice(raw).map_err(|_| Fail::ClientDataJson)?;
    let obj = v.as_object().ok_or(Fail::ClientDataJson)?;
    if obj.get("type").and_then(|t| t.as_str()) != Some(want_type) {
        return Err(Fail::Type);
    }
    let got = obj.get("challenge").and_then(|c| c.as_str()).ok_or(Fail::Challenge)?;
    let got = b64url_decode(got, 64).map_err(|_| Fail::Challenge)?;
    if !same_bytes(&got, challenge) {
        return Err(Fail::Challenge);
    }
    if obj.get("origin").and_then(|o| o.as_str()) != Some(origin) {
        return Err(Fail::Origin);
    }
    match obj.get("crossOrigin") {
        None | Some(serde_json::Value::Bool(false)) => {}
        _ => return Err(Fail::CrossOrigin),
    }
    if obj.contains_key("topOrigin") {
        return Err(Fail::TopOrigin);
    }
    Ok(())
}

/// The origin of RP ID `rp_id`.
pub fn origin_of(rp_id: &str) -> String {
    format!("https://{rp_id}")
}

fn flags_ok(a: &AuthData, rp_id: &str) -> Result<(), Fail> {
    if a.rp_id_hash[..] != Sha256::digest(rp_id.as_bytes())[..] {
        return Err(Fail::RpIdHash);
    }
    if a.flags & FLAG_UP == 0 {
        return Err(Fail::UserPresent);
    }
    if a.flags & FLAG_UV == 0 {
        return Err(Fail::UserVerified);
    }
    Ok(())
}

// ── The two ceremonies ───────────────────────────────────────────────────

/// What a registration yields: stored per device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCredential {
    pub id: Vec<u8>,
    /// Uncompressed SEC1 point.
    pub public_key: Vec<u8>,
    pub counter: u32,
}

/// Verify `navigator.credentials.create` (attestation `none`, ES256).
/// `raw_id` is the `rawId` the page sent; it must be the attested one.
pub fn verify_registration(rp_id: &str, challenge: &[u8], raw_id: &[u8], client_data: &[u8], attestation_object: &[u8]) -> Result<NewCredential, Fail> {
    check_client_data(client_data, "webauthn.create", challenge, &origin_of(rp_id))?;
    if attestation_object.len() > MAX_ATTESTATION {
        return Err(Fail::Cbor);
    }
    let att = cbor::decode_exact(attestation_object).map_err(|_| Fail::Cbor)?;
    let Value::Map(m) = &att else { return Err(Fail::Cbor) };
    if m.len() != 3 {
        return Err(Fail::Attestation);
    }
    if att.get_text("fmt") != Some(&Value::Text("none")) || att.get_text("attStmt") != Some(&Value::Map(vec![])) {
        return Err(Fail::Attestation);
    }
    let Some(Value::Bytes(auth)) = att.get_text("authData") else { return Err(Fail::Attestation) };
    let a = parse_auth_data(auth)?;
    flags_ok(&a, rp_id)?;
    if a.flags & FLAG_AT == 0 {
        return Err(Fail::AttestedCredential);
    }
    let (id, public_key) = a.attested.ok_or(Fail::AttestedCredential)?;
    if !same_bytes(&id, raw_id) {
        return Err(Fail::CredentialId);
    }
    Ok(NewCredential { id, public_key, counter: a.counter })
}

/// Verify `navigator.credentials.get` against a stored key and counter.
/// The new counter to store on success. A counter of 0 is always accepted
/// (iCloud Keychain passkeys never count, and it leaves the stored one); a
/// non-zero one must be above `stored_counter`.
pub fn verify_assertion(rp_id: &str, challenge: &[u8], public_key: &[u8], stored_counter: u32, client_data: &[u8], auth_data: &[u8], signature: &[u8]) -> Result<u32, Fail> {
    check_client_data(client_data, "webauthn.get", challenge, &origin_of(rp_id))?;
    if auth_data.len() > MAX_AUTH_DATA {
        return Err(Fail::AuthData);
    }
    let a = parse_auth_data(auth_data)?;
    flags_ok(&a, rp_id)?;
    if a.attested.is_some() {
        return Err(Fail::AttestedCredential);
    }
    if signature.is_empty() || signature.len() > MAX_SIGNATURE {
        return Err(Fail::Signature);
    }
    let key = VerifyingKey::from_sec1_bytes(public_key).map_err(|_| Fail::KeyPoint)?;
    let sig = Signature::from_der(signature).map_err(|_| Fail::Signature)?;
    let mut signed = Vec::with_capacity(auth_data.len() + 32);
    signed.extend_from_slice(auth_data);
    signed.extend_from_slice(&Sha256::digest(client_data));
    key.verify(&signed, &sig).map_err(|_| Fail::Signature)?;
    match a.counter {
        0 => Ok(stored_counter),
        n if n > stored_counter => Ok(n),
        _ => Err(Fail::CounterRegression),
    }
}

/// A software authenticator, for tests only (this crate's and the daemon's,
/// through the `test-authenticator` feature): it makes the bytes a browser
/// and a platform authenticator would, and lets a test break each one.
#[cfg(any(test, feature = "test-authenticator"))]
pub mod soft {
    use super::*;
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::SigningKey;

    pub struct Authenticator {
        pub key: SigningKey,
        pub id: Vec<u8>,
        pub counter: u32,
    }

    /// What a test can change in one ceremony.
    #[derive(Clone)]
    pub struct Twist {
        pub typ: Option<String>,
        pub origin: Option<String>,
        pub challenge: Option<Vec<u8>>,
        pub rp_id: Option<String>,
        pub flags: u8,
        pub cross_origin: Option<serde_json::Value>,
        pub top_origin: Option<String>,
        pub alg: i128,
        pub fmt: String,
        /// Sign with another key than the one registered.
        pub other_key: bool,
        /// Flip a bit of the signature.
        pub bad_signature: bool,
        pub counter: Option<u32>,
    }

    impl Default for Twist {
        fn default() -> Self {
            Twist {
                typ: None,
                origin: None,
                challenge: None,
                rp_id: None,
                flags: FLAG_UP | FLAG_UV,
                cross_origin: None,
                top_origin: None,
                alg: COSE_ALG_ES256,
                fmt: "none".into(),
                other_key: false,
                bad_signature: false,
                counter: None,
            }
        }
    }

    fn random(n: usize) -> Vec<u8> {
        let mut b = vec![0u8; n];
        getrandom::getrandom(&mut b).expect("randomness");
        b
    }

    fn new_key() -> SigningKey {
        loop {
            if let Ok(k) = SigningKey::from_slice(&random(32)) {
                return k;
            }
        }
    }

    pub fn client_data(typ: &str, challenge: &[u8], rp_id: &str, t: &Twist) -> Vec<u8> {
        let mut v = serde_json::json!({
            "type": t.typ.clone().unwrap_or_else(|| typ.to_string()),
            "challenge": b64url_encode(t.challenge.as_deref().unwrap_or(challenge)),
            "origin": t.origin.clone().unwrap_or_else(|| origin_of(rp_id)),
        });
        if let Some(c) = &t.cross_origin {
            v["crossOrigin"] = c.clone();
        }
        if let Some(o) = &t.top_origin {
            v["topOrigin"] = serde_json::Value::String(o.clone());
        }
        serde_json::to_vec(&v).unwrap()
    }

    /// CBOR of the few shapes, written by hand like the decoder reads them.
    fn head(major: u8, n: u64, out: &mut Vec<u8>) {
        let m = major << 5;
        match n {
            0..=23 => out.push(m | n as u8),
            24..=0xff => out.extend([m | 24, n as u8]),
            0x100..=0xffff => {
                out.push(m | 25);
                out.extend((n as u16).to_be_bytes());
            }
            _ => {
                out.push(m | 26);
                out.extend((n as u32).to_be_bytes());
            }
        }
    }
    pub fn int(n: i128, out: &mut Vec<u8>) {
        if n >= 0 {
            head(0, n as u64, out)
        } else {
            head(1, (-1 - n) as u64, out)
        }
    }
    pub fn bytes(b: &[u8], out: &mut Vec<u8>) {
        head(2, b.len() as u64, out);
        out.extend_from_slice(b);
    }
    pub fn text(s: &str, out: &mut Vec<u8>) {
        head(3, s.len() as u64, out);
        out.extend_from_slice(s.as_bytes());
    }
    pub fn map(n: u64, out: &mut Vec<u8>) {
        head(5, n, out)
    }

    pub fn cose_key(key: &SigningKey, alg: i128) -> Vec<u8> {
        let p = VerifyingKey::from(key).to_encoded_point(false);
        let mut out = vec![];
        map(5, &mut out);
        int(1, &mut out);
        int(COSE_KTY_EC2, &mut out);
        int(3, &mut out);
        int(alg, &mut out);
        int(-1, &mut out);
        int(COSE_CRV_P256, &mut out);
        int(-2, &mut out);
        bytes(p.x().unwrap(), &mut out);
        int(-3, &mut out);
        bytes(p.y().unwrap(), &mut out);
        out
    }

    impl Authenticator {
        pub fn new() -> Self {
            Authenticator { key: new_key(), id: random(32), counter: 0 }
        }

        pub fn auth_data(&self, rp_id: &str, flags: u8, counter: u32, attested: Option<&[u8]>) -> Vec<u8> {
            let mut a = Sha256::digest(rp_id.as_bytes()).to_vec();
            a.push(flags | if attested.is_some() { FLAG_AT } else { 0 });
            a.extend(counter.to_be_bytes());
            if let Some(key) = attested {
                a.extend([0u8; 16]);
                a.extend((self.id.len() as u16).to_be_bytes());
                a.extend_from_slice(&self.id);
                a.extend_from_slice(key);
            }
            a
        }

        /// `(rawId, clientDataJSON, attestationObject)` for a registration.
        pub fn register(&self, rp_id: &str, challenge: &[u8], t: &Twist) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
            let cd = client_data("webauthn.create", challenge, rp_id, t);
            let key = cose_key(&self.key, t.alg);
            let ad = self.auth_data(t.rp_id.as_deref().unwrap_or(rp_id), t.flags, t.counter.unwrap_or(self.counter), Some(&key));
            let mut att = vec![];
            map(3, &mut att);
            text("fmt", &mut att);
            text(&t.fmt, &mut att);
            text("attStmt", &mut att);
            map(0, &mut att);
            text("authData", &mut att);
            bytes(&ad, &mut att);
            (self.id.clone(), cd, att)
        }

        /// `(rawId, clientDataJSON, authenticatorData, signature)` for an
        /// assertion; the counter moves on as a counting authenticator's would.
        pub fn assert(&mut self, rp_id: &str, challenge: &[u8], t: &Twist) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
            if t.counter.is_none() && self.counter > 0 {
                self.counter += 1;
            }
            let cd = client_data("webauthn.get", challenge, rp_id, t);
            let ad = self.auth_data(t.rp_id.as_deref().unwrap_or(rp_id), t.flags, t.counter.unwrap_or(self.counter), None);
            let mut signed = ad.clone();
            signed.extend_from_slice(&Sha256::digest(&cd));
            let key = if t.other_key { new_key() } else { self.key.clone() };
            let sig: Signature = key.sign(&signed);
            let mut der = sig.to_der().as_bytes().to_vec();
            if t.bad_signature {
                let n = der.len() - 1;
                der[n] ^= 1;
            }
            (self.id.clone(), cd, ad, der)
        }
    }

    impl Default for Authenticator {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::soft::{Authenticator, Twist};
    use super::*;

    const RP: &str = "sbxw.tail1.ts.net";

    #[test]
    fn base64url_is_strict() {
        for (raw, text) in [(&b""[..], ""), (b"f", "Zg"), (b"fo", "Zm8"), (b"foo", "Zm9v"), (b"\xfb\xff", "-_8")] {
            assert_eq!(b64url_encode(raw), text);
            assert_eq!(b64url_decode(text, 16).unwrap(), raw);
        }
        for bad in ["Zg==", "Zg=", "Z", "Zm9v=", "Zm 9", "Zm+v", "Zm/v", "Zh", "Zm9", "é", "Zm9\n"] {
            // "Zh": trailing bits not zero; "Zm9" decodes "fo" only with zero bits ("Zm8" does).
            assert!(b64url_decode(bad, 16).is_err(), "{bad:?}");
        }
        assert!(b64url_decode("AAAA", 2).is_err(), "over the cap");
        assert!(b64url_decode("AAAA", 3).is_ok());
        let mut rng = super::super::cbor::tests_rng();
        for _ in 0..2000 {
            let n = rng.next() as usize % 50;
            let raw: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
            assert_eq!(b64url_decode(&b64url_encode(&raw), 64).unwrap(), raw);
            let junk: String = (0..n).map(|_| (rng.next() % 128) as u8 as char).collect();
            let _ = b64url_decode(&junk, 64);
        }
    }

    fn registered() -> (Authenticator, NewCredential) {
        let a = Authenticator::new();
        let ch = [7u8; 32];
        let (id, cd, att) = a.register(RP, &ch, &Twist::default());
        let c = verify_registration(RP, &ch, &id, &cd, &att).expect("a good registration");
        (a, c)
    }

    #[test]
    fn a_good_registration_and_assertion_pass() {
        let (mut a, c) = registered();
        assert_eq!(c.id, a.id);
        assert_eq!(c.public_key.len(), 65);
        let ch = [9u8; 32];
        let (_, cd, ad, sig) = a.assert(RP, &ch, &Twist::default());
        assert_eq!(verify_assertion(RP, &ch, &c.public_key, 0, &cd, &ad, &sig), Ok(0));
    }

    /// Each registration check, removed, would let this input through.
    #[test]
    fn every_registration_check_refuses_its_forgery() {
        let a = Authenticator::new();
        let ch = [7u8; 32];
        let t = Twist::default;
        let cases: Vec<(Twist, Fail)> = vec![
            (Twist { typ: Some("webauthn.get".into()), ..t() }, Fail::Type),
            (Twist { challenge: Some(vec![8u8; 32]), ..t() }, Fail::Challenge),
            (Twist { origin: Some("https://evil.ts.net".into()), ..t() }, Fail::Origin),
            (Twist { origin: Some(format!("http://{RP}")), ..t() }, Fail::Origin),
            (Twist { origin: Some(format!("https://{RP}:443")), ..t() }, Fail::Origin),
            (Twist { cross_origin: Some(serde_json::json!(true)), ..t() }, Fail::CrossOrigin),
            (Twist { cross_origin: Some(serde_json::json!("false")), ..t() }, Fail::CrossOrigin),
            (Twist { top_origin: Some(format!("https://{RP}")), ..t() }, Fail::TopOrigin),
            (Twist { rp_id: Some("evil.ts.net".into()), ..t() }, Fail::RpIdHash),
            (Twist { flags: FLAG_UV, ..t() }, Fail::UserPresent),
            (Twist { flags: FLAG_UP, ..t() }, Fail::UserVerified),
            (Twist { alg: -257, ..t() }, Fail::KeyShape),
            (Twist { alg: -8, ..t() }, Fail::KeyShape),
            (Twist { fmt: "packed".into(), ..t() }, Fail::Attestation),
        ];
        for (twist, want) in cases {
            let (id, cd, att) = a.register(RP, &ch, &twist);
            assert_eq!(verify_registration(RP, &ch, &id, &cd, &att), Err(want), "{want:?}");
        }
        let (id, cd, att) = a.register(RP, &ch, &t());
        assert_eq!(verify_registration(RP, &ch, &[1u8; 32], &cd, &att), Err(Fail::CredentialId), "rawId not the attested id");
        assert_eq!(verify_registration("other.ts.net", &ch, &id, &cd, &att), Err(Fail::Origin), "another RP");
        assert!(verify_registration(RP, &ch, &id, &cd, &att).is_ok());
    }

    #[test]
    fn a_cose_key_off_the_curve_or_off_shape_is_refused() {
        use super::soft::{bytes, int, map};
        let x = [1u8; 32];
        let y = [2u8; 32];
        let enc = |crv: i128, xlen: usize, extra: Option<(i128, i128)>| {
            let mut out = vec![];
            map(if extra.is_some() { 6 } else { 5 }, &mut out);
            for (k, v) in [(1, 2), (3, -7), (-1, crv)] {
                int(k, &mut out);
                int(v, &mut out);
            }
            int(-2, &mut out);
            bytes(&x[..xlen], &mut out);
            int(-3, &mut out);
            bytes(&y, &mut out);
            if let Some((k, v)) = extra {
                int(k, &mut out);
                int(v, &mut out);
            }
            out
        };
        let check = |b: Vec<u8>| cose_es256(&cbor::decode_exact(&b).unwrap());
        assert_eq!(check(enc(1, 32, None)), Err(Fail::KeyPoint), "(1,1..,2,2..) is not on P-256");
        assert_eq!(check(enc(2, 32, None)), Err(Fail::KeyShape), "P-384");
        assert_eq!(check(enc(1, 31, None)), Err(Fail::KeyShape), "short x");
        assert_eq!(check(enc(1, 32, Some((4, 1)))), Err(Fail::KeyShape), "an extra key");
        let a = Authenticator::new();
        assert!(check(super::soft::cose_key(&a.key, -7)).is_ok());
    }

    /// Each assertion check, removed, would let this input through.
    #[test]
    fn every_assertion_check_refuses_its_forgery() {
        let (mut a, c) = registered();
        let ch = [9u8; 32];
        let t = Twist::default;
        let cases: Vec<(Twist, Fail)> = vec![
            (Twist { typ: Some("webauthn.create".into()), ..t() }, Fail::Type),
            (Twist { challenge: Some(vec![1u8; 32]), ..t() }, Fail::Challenge),
            (Twist { origin: Some("https://sbxw.tail2.ts.net".into()), ..t() }, Fail::Origin),
            (Twist { cross_origin: Some(serde_json::json!(true)), ..t() }, Fail::CrossOrigin),
            (Twist { top_origin: Some("https://x".into()), ..t() }, Fail::TopOrigin),
            (Twist { rp_id: Some("sbxw.tail2.ts.net".into()), ..t() }, Fail::RpIdHash),
            (Twist { flags: FLAG_UV, ..t() }, Fail::UserPresent),
            (Twist { flags: FLAG_UP, ..t() }, Fail::UserVerified),
            (Twist { other_key: true, ..t() }, Fail::Signature),
            (Twist { bad_signature: true, ..t() }, Fail::Signature),
        ];
        for (twist, want) in cases {
            let (_, cd, ad, sig) = a.assert(RP, &ch, &twist);
            assert_eq!(verify_assertion(RP, &ch, &c.public_key, 0, &cd, &ad, &sig), Err(want), "{want:?}");
        }
        // A signature over other bytes than these: the challenge swapped after signing.
        let (_, cd, ad, sig) = a.assert(RP, &ch, &t());
        let other = client_data_with(&cd, "challenge", &b64url_encode(&[3u8; 32]));
        assert_eq!(verify_assertion(RP, &[3u8; 32], &c.public_key, 0, &other, &ad, &sig), Err(Fail::Signature));
        // authData with an attested credential is not an assertion's.
        let ad_at = a.auth_data(RP, FLAG_UP | FLAG_UV, 0, Some(&super::soft::cose_key(&a.key, -7)));
        assert_eq!(verify_assertion(RP, &ch, &c.public_key, 0, &cd, &ad_at, &sig), Err(Fail::AttestedCredential));
        assert_eq!(verify_assertion(RP, &ch, &c.public_key, 0, &cd, &ad, &[]), Err(Fail::Signature));
        assert_eq!(verify_assertion(RP, &ch, &c.public_key, 0, &cd, &ad, &[0x30; 81]), Err(Fail::Signature));
        assert_eq!(verify_assertion(RP, &ch, &[4u8; 65], 0, &cd, &ad, &sig), Err(Fail::KeyPoint));
    }

    fn client_data_with(cd: &[u8], k: &str, v: &str) -> Vec<u8> {
        let mut j: serde_json::Value = serde_json::from_slice(cd).unwrap();
        j[k] = serde_json::Value::String(v.into());
        serde_json::to_vec(&j).unwrap()
    }

    #[test]
    fn counters_zero_always_else_increasing() {
        let (mut a, c) = registered();
        let ch = [9u8; 32];
        let mut at = |n: u32, stored: u32| {
            let (_, cd, ad, sig) = a.assert(RP, &ch, &Twist { counter: Some(n), ..Twist::default() });
            verify_assertion(RP, &ch, &c.public_key, stored, &cd, &ad, &sig)
        };
        assert_eq!(at(0, 0), Ok(0));
        assert_eq!(at(0, 41), Ok(41), "0 is always accepted, the stored counter stays");
        assert_eq!(at(5, 4), Ok(5));
        assert_eq!(at(5, 5), Err(Fail::CounterRegression));
        assert_eq!(at(3, 5), Err(Fail::CounterRegression));
    }

    /// authenticatorData: fixed layout, checked offsets, no panic.
    #[test]
    fn auth_data_parser_refuses_hostile_layouts() {
        let a = Authenticator::new();
        let key = super::soft::cose_key(&a.key, -7);
        let good = a.auth_data(RP, FLAG_UP | FLAG_UV, 1, Some(&key));
        assert!(parse_auth_data(&good).is_ok());
        let plain = a.auth_data(RP, FLAG_UP | FLAG_UV, 1, None);
        assert!(parse_auth_data(&plain).is_ok());
        let mut extra = plain.clone();
        extra.push(0);
        assert_eq!(parse_auth_data(&extra), Err(Fail::AuthData), "a byte past the end");
        let mut ed = plain.clone();
        ed[32] |= FLAG_ED;
        assert_eq!(parse_auth_data(&ed), Err(Fail::Cbor), "ED with no map");
        ed.extend([0xa1, 0x61, b'x', 0xf5]);
        assert!(parse_auth_data(&ed).is_ok(), "ED with one map");
        ed.push(0);
        assert_eq!(parse_auth_data(&ed), Err(Fail::AuthData));
        let mut long_id = good.clone();
        long_id[53] = 0xff;
        long_id[54] = 0xff;
        assert_eq!(parse_auth_data(&long_id), Err(Fail::CredentialId));
        let mut short_id = good.clone();
        short_id[53] = 0;
        short_id[54] = 1;
        assert_eq!(parse_auth_data(&short_id), Err(Fail::CredentialId));
        for n in 0..good.len() {
            assert!(parse_auth_data(&good[..n]).is_err(), "prefix {n}");
        }
        let mut rng = super::super::cbor::tests_rng();
        for i in 0..5000 {
            let mut m = if i % 2 == 0 { good.clone() } else { plain.clone() };
            for _ in 0..1 + rng.next() % 3 {
                let at = rng.next() as usize % m.len();
                match rng.next() % 3 {
                    0 => m[at] = rng.next() as u8,
                    1 => m.truncate(at.max(1)),
                    _ => m.insert(at, rng.next() as u8),
                }
            }
            let _ = parse_auth_data(&m);
            let _ = verify_assertion(RP, &[0u8; 32], &[4u8; 65], 0, b"{}", &m, &[0x30, 0]);
        }
    }

    #[test]
    fn malformed_client_data_and_attestation_never_panic() {
        let ch = [7u8; 32];
        for cd in [&b""[..], b"null", b"[]", b"{\"type\":1}", b"{\"type\":\"webauthn.create\"}", b"{\"type\":\"webauthn.create\",\"challenge\":\"!!\"}", &[0xff; 3000][..]] {
            assert!(check_client_data(cd, "webauthn.create", &ch, "https://x").is_err());
        }
        let a = Authenticator::new();
        let (id, cd, att) = a.register(RP, &ch, &Twist::default());
        let mut rng = super::super::cbor::tests_rng();
        for _ in 0..3000 {
            let mut m = att.clone();
            let at = rng.next() as usize % m.len();
            m[at] = rng.next() as u8;
            if rng.next().is_multiple_of(2) {
                m.truncate(rng.next() as usize % m.len());
            }
            let r = verify_registration(RP, &ch, &id, &cd, &m);
            if r.is_ok() {
                // A flipped byte inside the AAGUID or the counter is harmless.
                assert_eq!(m.len(), att.len());
            }
        }
        assert_eq!(verify_registration(RP, &ch, &id, &cd, &vec![0u8; MAX_ATTESTATION + 1]), Err(Fail::Cbor));
    }
}
