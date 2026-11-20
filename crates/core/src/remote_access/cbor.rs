//! A strict CBOR (RFC 8949) decoder for the few shapes WebAuthn hands the
//! daemon: the attestation object (a text-keyed map), a COSE key (an
//! integer-keyed map) and an optional extensions map. Input is untrusted.
//!
//! Refused, never guessed: indefinite lengths, tags, floats, every simple
//! value but false, true and null, reserved additional-info values, a length
//! past the end of the input, a nesting deeper than [`MAX_DEPTH`], more than
//! [`MAX_ITEMS`] entries in one array or map, a duplicate map key, and (for
//! [`decode_exact`]) any byte after the item. Nothing is allocated before
//! the bytes it describes are known to be there: an array or map's count is
//! checked against [`MAX_ITEMS`] and against the bytes left (each entry takes
//! at least one) before any `Vec` is sized from it. Recursion is bounded by
//! [`MAX_DEPTH`]; every offset is computed with checked arithmetic.

#![forbid(unsafe_code)]

pub const MAX_DEPTH: usize = 4;
pub const MAX_ITEMS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value<'a> {
    Uint(u64),
    /// `-1 - n`, kept wide so every CBOR negative fits.
    Neg(i128),
    Bytes(&'a [u8]),
    Text(&'a str),
    Array(Vec<Value<'a>>),
    Map(Vec<(Value<'a>, Value<'a>)>),
    Bool(bool),
    Null,
}

impl<'a> Value<'a> {
    /// An integer of either sign.
    pub fn int(&self) -> Option<i128> {
        match self {
            Value::Uint(n) => Some(*n as i128),
            Value::Neg(n) => Some(*n),
            _ => None,
        }
    }

    /// The value at text key `k` of a map.
    pub fn get_text(&self, k: &str) -> Option<&Value<'a>> {
        match self {
            Value::Map(m) => m.iter().find(|(key, _)| matches!(key, Value::Text(t) if *t == k)).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value at integer key `k` of a map.
    pub fn get_int(&self, k: i128) -> Option<&Value<'a>> {
        match self {
            Value::Map(m) => m.iter().find(|(key, _)| key.int() == Some(k)).map(|(_, v)| v),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The input ended inside an item.
    Truncated,
    /// Indefinite length, a tag, a float, a reserved value.
    Unsupported,
    TooDeep,
    TooMany,
    DuplicateKey,
    /// Text that is not UTF-8.
    BadText,
    /// Bytes after the item.
    Trailing,
}

/// One item that must fill `input` exactly.
pub fn decode_exact(input: &[u8]) -> Result<Value<'_>, Error> {
    let (v, used) = decode_prefix(input)?;
    if used != input.len() {
        return Err(Error::Trailing);
    }
    Ok(v)
}

/// One item at the start of `input`, and how many bytes it took.
pub fn decode_prefix(input: &[u8]) -> Result<(Value<'_>, usize), Error> {
    let mut at = 0;
    let v = item(input, &mut at, 0)?;
    Ok((v, at))
}

fn take<'a>(input: &'a [u8], at: &mut usize, n: u64) -> Result<&'a [u8], Error> {
    let n = usize::try_from(n).map_err(|_| Error::Truncated)?;
    let end = at.checked_add(n).ok_or(Error::Truncated)?;
    let s = input.get(*at..end).ok_or(Error::Truncated)?;
    *at = end;
    Ok(s)
}

/// The argument of an initial byte's additional info.
fn argument(input: &[u8], at: &mut usize, info: u8) -> Result<u64, Error> {
    Ok(match info {
        0..=23 => info as u64,
        24 => take(input, at, 1)?[0] as u64,
        25 => u16::from_be_bytes(take(input, at, 2)?.try_into().expect("2")) as u64,
        26 => u32::from_be_bytes(take(input, at, 4)?.try_into().expect("4")) as u64,
        27 => u64::from_be_bytes(take(input, at, 8)?.try_into().expect("8")),
        // 28-30 reserved, 31 indefinite length.
        _ => return Err(Error::Unsupported),
    })
}

fn item<'a>(input: &'a [u8], at: &mut usize, depth: usize) -> Result<Value<'a>, Error> {
    let first = *take(input, at, 1)?.first().expect("1");
    let (major, info) = (first >> 5, first & 0x1f);
    if major == 7 {
        return match info {
            20 => Ok(Value::Bool(false)),
            21 => Ok(Value::Bool(true)),
            22 => Ok(Value::Null),
            _ => Err(Error::Unsupported),
        };
    }
    let arg = argument(input, at, info)?;
    match major {
        0 => Ok(Value::Uint(arg)),
        1 => Ok(Value::Neg(-1i128 - i128::from(arg))),
        2 => Ok(Value::Bytes(take(input, at, arg)?)),
        3 => std::str::from_utf8(take(input, at, arg)?).map(Value::Text).map_err(|_| Error::BadText),
        4 | 5 => {
            if depth + 1 > MAX_DEPTH {
                return Err(Error::TooDeep);
            }
            if arg > MAX_ITEMS as u64 {
                return Err(Error::TooMany);
            }
            let left = input.len().saturating_sub(*at) as u64;
            let least = if major == 4 { arg } else { arg.checked_mul(2).ok_or(Error::Truncated)? };
            if least > left {
                return Err(Error::Truncated);
            }
            if major == 4 {
                let mut out = Vec::with_capacity(arg as usize);
                for _ in 0..arg {
                    out.push(item(input, at, depth + 1)?);
                }
                Ok(Value::Array(out))
            } else {
                let mut out: Vec<(Value, Value)> = Vec::with_capacity(arg as usize);
                for _ in 0..arg {
                    let k = item(input, at, depth + 1)?;
                    let v = item(input, at, depth + 1)?;
                    if out.iter().any(|(seen, _)| *seen == k) {
                        return Err(Error::DuplicateKey);
                    }
                    out.push((k, v));
                }
                Ok(Value::Map(out))
            }
        }
        // 6: tags.
        _ => Err(Error::Unsupported),
    }
}

/// xorshift64: a fixed seed, the same inputs on every run (tests only).
#[cfg(test)]
pub(crate) struct Rng(u64);

#[cfg(test)]
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[cfg(test)]
pub(crate) fn tests_rng() -> Rng {
    Rng(0x2545_F491_4F6C_DD1D)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn the_shapes_webauthn_uses_decode() {
        // {"fmt": "none", "attStmt": {}, "authData": h'0102'}
        let att = hex("a363666d74646e6f6e656761747453746d74a0686175746844617461420102");
        let v = decode_exact(&att).unwrap();
        assert_eq!(v.get_text("fmt"), Some(&Value::Text("none")));
        assert_eq!(v.get_text("attStmt"), Some(&Value::Map(vec![])));
        assert_eq!(v.get_text("authData"), Some(&Value::Bytes(&[1, 2])));
        // {1: 2, 3: -7, -1: 1}
        let key = hex("a30102032620 01".replace(' ', "").as_str());
        let k = decode_exact(&key).unwrap();
        assert_eq!(k.get_int(1).and_then(Value::int), Some(2));
        assert_eq!(k.get_int(3).and_then(Value::int), Some(-7));
        assert_eq!(k.get_int(-1).and_then(Value::int), Some(1));
        assert_eq!(decode_exact(&hex("f4f5")), Err(Error::Trailing));
        assert_eq!(decode_prefix(&hex("f4f5")).unwrap(), (Value::Bool(false), 1));
        assert_eq!(decode_exact(&hex("f6")).unwrap(), Value::Null);
        assert_eq!(decode_exact(&hex("3bffffffffffffffff")).unwrap(), Value::Neg(-1 - u64::MAX as i128));
    }

    /// Malformed, hostile and oversized inputs: an error, never a panic.
    #[test]
    fn hostile_inputs_are_refused_without_panic() {
        let cases: &[(&str, Error)] = &[
            ("", Error::Truncated),
            ("18", Error::Truncated),
            ("19 01", Error::Truncated),
            ("1b 00 00 00 00", Error::Truncated),
            ("44 01 02", Error::Truncated),
            ("5b ff ff ff ff ff ff ff ff", Error::Truncated),
            ("7b 7f ff ff ff ff ff ff ff 00", Error::Truncated),
            ("62 ff fe", Error::BadText),
            ("5f 41 00 ff", Error::Unsupported),
            ("9f ff", Error::Unsupported),
            ("bf ff", Error::Unsupported),
            ("c0 60", Error::Unsupported),
            ("f9 00 00", Error::Unsupported),
            ("fa 00 00 00 00", Error::Unsupported),
            ("fb 00 00 00 00 00 00 00 00", Error::Unsupported),
            ("f7", Error::Unsupported),
            ("e0", Error::Unsupported),
            ("1c", Error::Unsupported),
            ("81 81 81 81 81 00", Error::TooDeep),
            ("98 21", Error::TooMany),
            ("b8 21", Error::TooMany),
            ("9b ff ff ff ff ff ff ff ff", Error::TooMany),
            ("a2 01 00 01 00", Error::DuplicateKey),
            ("a2 61 61 00 61 61 00", Error::DuplicateKey),
            ("a1 01", Error::Truncated),
            ("00 00", Error::Trailing),
        ];
        for (input, want) in cases {
            assert_eq!(decode_exact(&hex(&input.replace(' ', ""))), Err(*want), "{input}");
        }
        // Every prefix of a valid item, and every single-byte input.
        let good = hex("a363666d74646e6f6e656761747453746d74a068617574684461746142 0102".replace(' ', "").as_str());
        for n in 0..good.len() {
            assert!(decode_exact(&good[..n]).is_err(), "prefix {n}");
        }
        for b in 0..=255u8 {
            let _ = decode_exact(&[b]);
            let _ = decode_exact(&[b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        }
        // Seeded, deterministic: random inputs and mutations of a valid one.
        // Never a panic; whatever is accepted used every byte.
        let mut rng = tests_rng();
        for i in 0..20_000 {
            let buf: Vec<u8> = if i % 2 == 0 {
                (0..rng.next() % 48).map(|_| rng.next() as u8).collect()
            } else {
                let mut m = good.clone();
                for _ in 0..1 + rng.next() % 4 {
                    let at = rng.next() as usize % m.len();
                    match rng.next() % 3 {
                        0 => m[at] = rng.next() as u8,
                        1 => m.truncate(at),
                        _ => m.insert(at, rng.next() as u8),
                    }
                    if m.is_empty() {
                        m.push(0);
                    }
                }
                m
            };
            if decode_exact(&buf).is_ok() {
                assert_eq!(decode_prefix(&buf).unwrap().1, buf.len(), "accepted with trailing bytes: {buf:02x?}");
            }
            if let Ok((_, used)) = decode_prefix(&buf) {
                assert!(used <= buf.len());
            }
        }
    }

}
