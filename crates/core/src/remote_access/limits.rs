//! Fixed-window rate limits: so many hits per source and so many in all, per
//! window. The source is a hint (see [`super::source_of`]): the global cap
//! holds whatever a caller says it is.

use std::collections::HashMap;

/// Sources remembered per window at most; past it, new sources share one.
const MAX_SOURCES: usize = 1024;

#[derive(Debug)]
pub struct Limiter {
    window_ms: u64,
    per_source: u32,
    global: u32,
    started: u64,
    total: u32,
    by_source: HashMap<String, u32>,
}

impl Limiter {
    pub fn new(window_ms: u64, per_source: u32, global: u32) -> Self {
        Limiter { window_ms, per_source, global, started: 0, total: 0, by_source: HashMap::new() }
    }

    fn roll(&mut self, now: u64) {
        if now >= self.started + self.window_ms || now < self.started {
            self.started = now;
            self.total = 0;
            self.by_source.clear();
        }
    }

    /// Milliseconds until the window ends.
    fn retry_after(&self, now: u64) -> u64 {
        (self.started + self.window_ms).saturating_sub(now).max(1)
    }

    fn key(&self, source: &str) -> String {
        if self.by_source.len() >= MAX_SOURCES && !self.by_source.contains_key(source) {
            "overflow".into()
        } else {
            source.to_string()
        }
    }

    /// Count one hit; `Err(retry_after_ms)` when over either cap (the hit is
    /// then not counted).
    pub fn hit(&mut self, source: &str, now: u64) -> Result<(), u64> {
        self.check(source, now)?;
        let k = self.key(source);
        *self.by_source.entry(k).or_insert(0) += 1;
        self.total += 1;
        Ok(())
    }

    /// Whether a hit now would be refused, without counting one.
    pub fn check(&mut self, source: &str, now: u64) -> Result<(), u64> {
        self.roll(now);
        let k = self.key(source);
        let mine = self.by_source.get(&k).copied().unwrap_or(0);
        if self.total >= self.global || mine >= self.per_source {
            return Err(self.retry_after(now));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_source_then_global_then_a_new_window() {
        let mut l = Limiter::new(60_000, 2, 3);
        assert!(l.hit("a", 0).is_ok());
        assert!(l.hit("a", 1).is_ok());
        assert_eq!(l.hit("a", 2), Err(59_998));
        assert!(l.hit("b", 3).is_ok());
        assert!(l.hit("c", 4).is_err(), "global cap, whatever the source says");
        assert!(l.check("c", 4).is_err());
        assert!(l.hit("a", 60_000).is_ok(), "next window");
    }

    #[test]
    fn many_sources_cannot_grow_memory() {
        let mut l = Limiter::new(60_000, 1, u32::MAX);
        for i in 0..MAX_SOURCES {
            assert!(l.hit(&i.to_string(), 0).is_ok());
        }
        assert!(l.hit("new-1", 0).is_ok());
        assert!(l.hit("new-2", 0).is_err(), "the overflow bucket is shared");
        assert!(l.by_source.len() <= MAX_SOURCES + 1);
    }
}
