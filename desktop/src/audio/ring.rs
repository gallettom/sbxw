//! A lock-free single-producer single-consumer ring of `f32` samples.
//!
//! The device callback is one end, the audio control thread the other. Both
//! ends are wait-free: no lock, no allocation, no system call, so the
//! real-time callback never waits on the control thread. Capacity is a power
//! of two; one end each, enforced by the types ([`Producer`], [`Consumer`] are
//! not `Clone`).

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Inner {
    buf: Box<[UnsafeCell<f32>]>,
    mask: usize,
    /// Next slot the consumer reads (only the consumer writes it).
    head: AtomicUsize,
    /// Next slot the producer writes (only the producer writes it).
    tail: AtomicUsize,
}

// The slots between head and tail belong to the consumer, the others to the
// producer; the Acquire/Release pairs on head and tail hand them over.
unsafe impl Sync for Inner {}
unsafe impl Send for Inner {}

/// The writing end.
pub struct Producer {
    inner: Arc<Inner>,
}

/// The reading end.
pub struct Consumer {
    inner: Arc<Inner>,
}

/// A ring holding at least `min_capacity` samples (rounded up to a power of two).
pub fn ring(min_capacity: usize) -> (Producer, Consumer) {
    let cap = min_capacity.max(2).next_power_of_two();
    let buf: Box<[UnsafeCell<f32>]> = (0..cap).map(|_| UnsafeCell::new(0.0)).collect();
    let inner = Arc::new(Inner { buf, mask: cap - 1, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) });
    (Producer { inner: inner.clone() }, Consumer { inner })
}

impl Inner {
    fn capacity(&self) -> usize {
        self.mask + 1
    }
}

impl Producer {
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Free slots now.
    pub fn free(&self) -> usize {
        let head = self.inner.head.load(Ordering::Acquire);
        let tail = self.inner.tail.load(Ordering::Relaxed);
        self.inner.capacity() - tail.wrapping_sub(head)
    }

    /// Write as much of `data` as fits; returns how many samples were written.
    pub fn push_slice(&mut self, data: &[f32]) -> usize {
        let n = data.len().min(self.free());
        let tail = self.inner.tail.load(Ordering::Relaxed);
        for (i, &x) in data[..n].iter().enumerate() {
            let slot = &self.inner.buf[tail.wrapping_add(i) & self.inner.mask];
            // SAFETY: slots from tail to head + capacity are the producer's alone.
            unsafe { *slot.get() = x };
        }
        self.inner.tail.store(tail.wrapping_add(n), Ordering::Release);
        n
    }
}

impl Consumer {
    /// Samples ready to read now.
    pub fn len(&self) -> usize {
        let tail = self.inner.tail.load(Ordering::Acquire);
        let head = self.inner.head.load(Ordering::Relaxed);
        tail.wrapping_sub(head)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read up to `out.len()` samples; returns how many were read.
    pub fn pop_slice(&mut self, out: &mut [f32]) -> usize {
        let n = out.len().min(self.len());
        let head = self.inner.head.load(Ordering::Relaxed);
        for (i, o) in out[..n].iter_mut().enumerate() {
            let slot = &self.inner.buf[head.wrapping_add(i) & self.inner.mask];
            // SAFETY: slots from head to tail are the consumer's alone.
            *o = unsafe { *slot.get() };
        }
        self.inner.head.store(head.wrapping_add(n), Ordering::Release);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_is_a_power_of_two_and_full_rings_refuse_more() {
        let (mut p, mut c) = ring(5);
        assert_eq!(p.capacity(), 8);
        assert_eq!(p.push_slice(&[1.0; 10]), 8);
        assert_eq!(p.push_slice(&[2.0]), 0);
        let mut out = [0.0; 3];
        assert_eq!(c.pop_slice(&mut out), 3);
        assert_eq!(p.free(), 3);
        assert_eq!(c.len(), 5);
    }

    #[test]
    fn order_is_kept_across_the_wrap() {
        let (mut p, mut c) = ring(8);
        let mut next = 0.0f32;
        let mut want = 0.0f32;
        let mut out = [0.0; 5];
        for _ in 0..100 {
            let chunk: Vec<f32> = (0..3).map(|i| next + i as f32).collect();
            let n = p.push_slice(&chunk);
            next += n as f32;
            let got = c.pop_slice(&mut out);
            for &x in &out[..got] {
                assert_eq!(x, want);
                want += 1.0;
            }
        }
        assert!(want > 250.0);
    }

    #[test]
    fn two_threads_see_every_sample_once_in_order() {
        let (mut p, mut c) = ring(64);
        const N: usize = 200_000;
        let writer = std::thread::spawn(move || {
            let mut i = 0usize;
            while i < N {
                let chunk: Vec<f32> = (i..(i + 17).min(N)).map(|v| v as f32).collect();
                i += p.push_slice(&chunk);
            }
        });
        let mut seen = 0usize;
        let mut out = [0.0f32; 13];
        while seen < N {
            let n = c.pop_slice(&mut out);
            for &x in &out[..n] {
                assert_eq!(x as usize, seen);
                seen += 1;
            }
        }
        writer.join().unwrap();
        assert!(c.is_empty());
    }
}
