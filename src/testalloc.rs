//! Test builds only: the system allocator, counting what each thread asks for. Robustness tests
//! use it to check that broken input (a length field saying 4 GB) never makes a parser reserve
//! a huge buffer, and that reading doesn't copy the same bytes over and over.
//!
//! Also home to `Bytes`, the deterministic pseudo-random generator those tests feed parsers with.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static LARGEST: Cell<usize> = const { Cell::new(0) };
    static TOTAL: Cell<usize> = const { Cell::new(0) };
}

fn note(n: usize) {
    // `try_with`: allocations while the thread is being torn down are simply not counted.
    let _ = LARGEST.try_with(|c| c.set(c.get().max(n)));
    let _ = TOTAL.try_with(|c| c.set(c.get().saturating_add(n)));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        note(l.size());
        unsafe { System.alloc(l) }
    }

    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        note(l.size());
        unsafe { System.alloc_zeroed(l) }
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, new_size: usize) -> *mut u8 {
        note(new_size);
        unsafe { System.realloc(p, l, new_size) }
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// What `f` allocated on this thread: (its result, the largest single allocation, all bytes
/// allocated together, counting each growth of a buffer as a new allocation of its new size).
pub fn measure<R>(f: impl FnOnce() -> R) -> (R, usize, usize) {
    let (l0, t0) = (LARGEST.with(|c| c.replace(0)), TOTAL.with(|c| c.replace(0)));
    let r = f();
    let (l, t) = (LARGEST.with(|c| c.replace(l0)), TOTAL.with(|c| c.replace(t0)));
    (r, l, t)
}

/// A small deterministic pseudo-random generator (xorshift64*), so a failing input can be made
/// again from its seed.
pub struct Bytes(u64);

impl Bytes {
    pub fn new(seed: u64) -> Bytes {
        Bytes(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number below `n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }

    /// Random bytes, fewer than `max` of them.
    pub fn noise(&mut self, max: usize) -> Vec<u8> {
        let n = self.below(max);
        self.bytes(n)
    }

    /// `good` broken in one of several ways: cut short, bytes flipped, bytes inserted or
    /// removed, or a stretch overwritten with 0xFF (huge length fields).
    pub fn mutate(&mut self, good: &[u8]) -> Vec<u8> {
        let mut v = good.to_vec();
        for _ in 0..1 + self.below(4) {
            match self.below(5) {
                0 => v.truncate(self.below(v.len() + 1)),
                1 if !v.is_empty() => {
                    let i = self.below(v.len());
                    v[i] ^= 1 << self.below(8);
                }
                2 => {
                    let i = self.below(v.len() + 1);
                    let extra = self.noise(8);
                    v.splice(i..i, extra);
                }
                3 if !v.is_empty() => {
                    let i = self.below(v.len());
                    let n = (1 + self.below(8)).min(v.len() - i);
                    v.drain(i..i + n);
                }
                _ if !v.is_empty() => {
                    let i = self.below(v.len());
                    let n = (1 + self.below(8)).min(v.len() - i);
                    v[i..i + n].fill(0xFF);
                }
                _ => {}
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocations_are_counted_per_thread() {
        let (v, largest, total) = measure(|| {
            let a = vec![0u8; 100_000];
            let b = vec![1u8; 10];
            a.len() + b.len()
        });
        assert_eq!(v, 100_010);
        assert!(largest >= 100_000 && largest < 200_000, "{largest}");
        assert!(total >= 100_010, "{total}");
        // Another thread's allocations don't count here.
        let (_, largest, _) = measure(|| std::thread::spawn(|| vec![0u8; 50_000_000].len()).join().unwrap());
        assert!(largest < 50_000_000, "{largest}");
    }

    #[test]
    fn the_generator_repeats_from_its_seed() {
        let (mut a, mut b) = (Bytes::new(7), Bytes::new(7));
        assert_eq!(a.bytes(64), b.bytes(64));
        assert_ne!(Bytes::new(8).bytes(64), Bytes::new(7).bytes(64));
        assert!((0..1000).all(|_| a.below(10) < 10));
        assert_eq!(a.below(0), 0);
        let good = b"hello, world".to_vec();
        for _ in 0..200 {
            let _ = a.mutate(&good);
        }
        assert!(a.mutate(&[]).len() <= 8 * 4);
    }
}
