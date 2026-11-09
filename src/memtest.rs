//! Long-lived memory upset detection ("scrubbing").
//!
//! A radiation-induced bit flip can only be observed if the bit holds known
//! data at the moment it is hit and is read back *afterwards*. The sensitive
//! area of a memory test is therefore `bits held × time held`, so this module
//! keeps a large, fixed pattern resident for the whole exposure and
//! periodically re-reads it (a "scrub"), correcting and reporting every
//! mismatch.
//!
//! The pattern is address-dependent (catches address-decoder faults and
//! aliasing) and contains a balanced mix of 0s and 1s. Its polarity is
//! inverted every `invert_every` scrubs so each cell is tested in both states
//! over a run (some cell types only upset 1→0 or 0→1).

use std::ptr;

/// One word that did not match its expected value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordUpset {
    /// Byte offset inside the region.
    pub offset: usize,
    pub expected: u64,
    pub actual: u64,
    /// True if this same word was also wrong in the previous scrub, even
    /// though it was rewritten — a hint of a stuck/hard (e.g. TID) fault
    /// rather than a transient upset.
    pub persistent: bool,
}

impl WordUpset {
    pub fn flipped_bits(&self) -> u32 {
        (self.expected ^ self.actual).count_ones()
    }
}

/// Result of one full pass over a region.
#[derive(Debug, Clone, Default)]
pub struct ScrubReport {
    pub scrub: u64,
    pub words_checked: usize,
    /// Total mismatching words (may exceed `upsets.len()` if capped).
    pub bad_words: usize,
    /// Total flipped bits across all bad words.
    pub bad_bits: u64,
    /// Detailed upsets, capped at `max_reports`.
    pub upsets: Vec<WordUpset>,
}

pub struct MemRegion {
    buf: Vec<u64>,
    scrubs: u64,
    invert_every: u64,
    max_reports: usize,
    /// Indices that failed in the previous scrub (to flag persistent faults).
    prev_bad: Vec<usize>,
}

/// Golden ratio constant: spreads consecutive indices over all bit patterns.
const MIX: u64 = 0x9E37_79B9_7F4A_7C15;

impl MemRegion {
    /// Allocate `bytes` (rounded down to whole words, minimum one word) and
    /// fill with the initial pattern.
    pub fn new(bytes: usize, invert_every: u64, max_reports: usize) -> Self {
        let words = (bytes / 8).max(1);
        let mut r = Self {
            buf: vec![0u64; words],
            scrubs: 0,
            invert_every: invert_every.max(1),
            max_reports,
            prev_bad: Vec::new(),
        };
        r.fill();
        r.lock();
        r
    }

    pub fn bytes(&self) -> usize {
        self.buf.len() * 8
    }

    pub fn scrubs(&self) -> u64 {
        self.scrubs
    }

    fn inverted(&self) -> bool {
        (self.scrubs / self.invert_every) % 2 == 1
    }

    /// Expected contents of word `i` for the current polarity.
    #[inline(always)]
    pub fn expected(&self, i: usize) -> u64 {
        // Bijective mix (splitmix64 finalizer): unique per address, ~50% ones.
        let mut base = (i as u64).wrapping_add(1).wrapping_mul(MIX);
        base ^= base >> 31;
        base = base.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        base ^= base >> 29;
        if self.inverted() { !base } else { base }
    }

    fn fill(&mut self) {
        for i in 0..self.buf.len() {
            let v = self.expected(i);
            // SAFETY: i is in bounds.
            unsafe { ptr::write_volatile(self.buf.as_mut_ptr().add(i), v) };
        }
    }

    /// Best effort: keep the region in RAM (no swap/compaction surprises).
    fn lock(&self) {
        #[cfg(unix)]
        unsafe {
            libc::mlock(self.buf.as_ptr() as *const libc::c_void, self.bytes());
        }
    }

    /// Read every word, report and correct mismatches, then (if the polarity
    /// period has elapsed) rewrite the region with the inverted pattern.
    pub fn scrub(&mut self) -> ScrubReport {
        let mut report = ScrubReport {
            scrub: self.scrubs,
            words_checked: self.buf.len(),
            ..Default::default()
        };
        let mut bad_now = Vec::new();
        let p = self.buf.as_mut_ptr();
        for i in 0..self.buf.len() {
            let expected = self.expected(i);
            // SAFETY: i is in bounds; volatile forces a real memory read.
            let actual = unsafe { ptr::read_volatile(p.add(i)) };
            if actual != expected {
                report.bad_words += 1;
                report.bad_bits += (actual ^ expected).count_ones() as u64;
                let persistent = self.prev_bad.binary_search(&i).is_ok();
                if report.upsets.len() < self.max_reports {
                    report.upsets.push(WordUpset {
                        offset: i * 8,
                        expected,
                        actual,
                        persistent,
                    });
                }
                bad_now.push(i);
                // Correct so the next upset in this word is counted fresh.
                unsafe { ptr::write_volatile(p.add(i), expected) };
            }
        }
        self.prev_bad = bad_now;

        let was_inverted = self.inverted();
        self.scrubs += 1;
        if self.inverted() != was_inverted {
            self.fill();
        }
        report
    }

    /// Test hook: flip one bit as if hit by a particle.
    pub fn flip_bit(&mut self, word: usize, bit: u32) {
        let i = word % self.buf.len();
        self.buf[i] ^= 1u64 << (bit % 64);
    }
}
