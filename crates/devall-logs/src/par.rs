//! Minimal scoped data parallelism (no thread-pool dependency).

use std::ops::Range;

pub(crate) fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8)
}

/// Splits `0..n` into contiguous chunks (at least `min_chunk` items each) and runs `f` on each in
/// parallel. Results come back in chunk order.
pub(crate) fn chunks<R: Send>(n: usize, min_chunk: usize, f: impl Fn(Range<usize>) -> R + Sync) -> Vec<R> {
    if n == 0 {
        return Vec::new();
    }
    let parts = threads().min(n.div_ceil(min_chunk.max(1))).max(1);
    if parts == 1 {
        return vec![f(0..n)];
    }
    let size = n.div_ceil(parts);
    let ranges: Vec<Range<usize>> = (0..parts).map(|i| i * size..((i + 1) * size).min(n)).filter(|r| !r.is_empty()).collect();
    let f = &f;
    std::thread::scope(|s| {
        let handles: Vec<_> = ranges.into_iter().map(|r| s.spawn(move || f(r))).collect();
        handles.into_iter().map(|h| h.join().expect("worker panicked")).collect()
    })
}
