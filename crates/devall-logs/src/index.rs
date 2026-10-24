//! Compact columns for per-line / per-entry data of multi-GB files.
//!
//! [`Packed`] stores i64 values as an i64 base per block of 1024 plus an i32 delta per value
//! (≈4 bytes/value instead of 8). Line offsets and timestamps are locally close, so deltas fit;
//! if one doesn't (a > 2 GiB block, timestamps > 24 days apart within a block) the column
//! transparently widens to plain i64s.

const BLOCK: usize = 1024;
/// Marker for "no value" (e.g. an entry without a timestamp).
pub const NONE: i64 = i64::MIN;
const NONE_REL: i32 = i32::MIN;

#[derive(Debug, Clone, Default)]
pub struct Packed {
    bases: Vec<i64>,
    rel: Vec<i32>,
    wide: Option<Vec<i64>>,
}

impl Packed {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        match &self.wide {
            Some(w) => w.len(),
            None => self.rel.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn push(&mut self, v: i64) {
        if let Some(w) = &mut self.wide {
            w.push(v);
            return;
        }
        let i = self.rel.len();
        if i.is_multiple_of(BLOCK) {
            self.bases.push(NONE);
        }
        let b = i / BLOCK;
        if v == NONE {
            self.rel.push(NONE_REL);
            return;
        }
        if self.bases[b] == NONE {
            self.bases[b] = v;
        }
        let d = v.wrapping_sub(self.bases[b]);
        if d > i32::MIN as i64 && d <= i32::MAX as i64 {
            self.rel.push(d as i32);
        } else {
            self.widen();
            self.wide.as_mut().unwrap().push(v);
        }
    }

    fn widen(&mut self) {
        let all: Vec<i64> = (0..self.rel.len()).map(|i| self.get(i)).collect();
        self.bases = Vec::new();
        self.rel = Vec::new();
        self.wide = Some(all);
    }

    #[inline]
    pub fn get(&self, i: usize) -> i64 {
        if let Some(w) = &self.wide {
            return w[i];
        }
        let r = self.rel[i];
        if r == NONE_REL { NONE } else { self.bases[i / BLOCK] + r as i64 }
    }

    pub fn last(&self) -> Option<i64> {
        let n = self.len();
        (n > 0).then(|| self.get(n - 1))
    }

    pub fn truncate(&mut self, n: usize) {
        if let Some(w) = &mut self.wide {
            w.truncate(n);
            return;
        }
        self.rel.truncate(n);
        self.bases.truncate(n.div_ceil(BLOCK));
    }

    pub fn shrink_to_fit(&mut self) {
        self.bases.shrink_to_fit();
        self.rel.shrink_to_fit();
        if let Some(w) = &mut self.wide {
            w.shrink_to_fit();
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Approximate heap size in bytes.
    pub fn heap_bytes(&self) -> usize {
        match &self.wide {
            Some(w) => w.capacity() * 8,
            None => self.bases.capacity() * 8 + self.rel.capacity() * 4,
        }
    }
}

/// A fixed-size bit set (filter results per source).
#[derive(Debug, Clone, Default)]
pub struct Bits {
    words: Vec<u64>,
    len: usize,
}

impl Bits {
    pub fn new(len: usize) -> Self {
        Self { words: vec![0; len.div_ceil(64)], len }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn set(&mut self, i: usize) {
        self.words[i / 64] |= 1 << (i % 64);
    }

    #[inline]
    pub fn get(&self, i: usize) -> bool {
        i < self.len && self.words[i / 64] & (1 << (i % 64)) != 0
    }

    /// Indices of set bits in ascending order.
    pub fn ones(&self) -> impl Iterator<Item = usize> + '_ {
        self.words.iter().enumerate().flat_map(|(wi, &w)| {
            let mut w = w;
            std::iter::from_fn(move || {
                if w == 0 {
                    return None;
                }
                let b = w.trailing_zeros() as usize;
                w &= w - 1;
                Some(wi * 64 + b)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_roundtrip_and_widen() {
        let mut p = Packed::new();
        let mut expect = Vec::new();
        for i in 0..5000i64 {
            let v = if i % 7 == 0 { NONE } else { 1_700_000_000_000 + i * 37 };
            p.push(v);
            expect.push(v);
        }
        assert!(p.wide.is_none());
        p.shrink_to_fit();
        assert!(p.heap_bytes() < 5000 * 5, "{}", p.heap_bytes());
        for (i, v) in expect.iter().enumerate() {
            assert_eq!(p.get(i), *v);
        }
        // A far-away value forces the wide representation; everything still reads back.
        p.push(5);
        expect.push(5);
        assert!(p.wide.is_some());
        for (i, v) in expect.iter().enumerate() {
            assert_eq!(p.get(i), *v);
        }
        p.truncate(10);
        assert_eq!(p.len(), 10);
        assert_eq!(p.last(), Some(expect[9]));
    }

    #[test]
    fn packed_truncate_then_push() {
        let mut p = Packed::new();
        for i in 0..2050 {
            p.push(i * 10);
        }
        p.truncate(1024);
        p.push(99_999);
        assert_eq!(p.get(1024), 99_999);
        assert_eq!(p.get(1023), 10_230);
    }

    #[test]
    fn bits() {
        let mut b = Bits::new(200);
        for i in [0, 63, 64, 130, 199] {
            b.set(i);
        }
        assert_eq!(b.ones().collect::<Vec<_>>(), vec![0, 63, 64, 130, 199]);
        assert!(b.get(130) && !b.get(131) && !b.get(500));
    }
}
