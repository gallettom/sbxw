//! Unguessable identifiers, from the OS.
//!
//! A plan id and an agent token are capabilities: whoever holds one can act
//! with it, so neither may be predictable from a clock or a counter.

/// `bytes` random bytes as lower-case hex.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).expect("the OS has no randomness to give");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// A plan id: 128 bits.
pub fn plan_id() -> String {
    format!("p-{}", random_hex(16))
}

/// A sandbox's agent token: 256 bits.
pub fn agent_token() -> String {
    format!("sbxw_{}", random_hex(32))
}

/// Constant-time comparison, for checking a presented token.
pub fn same_secret(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_long_distinct_and_prefixed() {
        let (a, b) = (agent_token(), agent_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 5 + 64);
        assert!(plan_id().starts_with("p-"));
        assert!(same_secret(&a, &a.clone()));
        assert!(!same_secret(&a, &b));
        assert!(!same_secret("x", "xy"));
    }
}
