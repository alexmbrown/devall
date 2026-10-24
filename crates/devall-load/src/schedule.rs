//! Load profiles as functions of time.
//!
//! * [`ArrivalSchedule`] turns a piecewise-linear iteration rate into the exact intended start
//!   offset of every iteration: iteration `k` starts when the integral of the rate reaches `k`.
//!   Latency in open-model tests is measured from these offsets, which is what makes the numbers
//!   free of coordinated omission.
//! * [`vus_at`] is the VU target of a ramping-VU profile at a point in time.

use std::time::Duration;

use crate::model::{Executor, per_sec};

#[derive(Debug, Clone, Copy)]
struct Seg {
    t0: f64,
    t1: f64,
    /// Rate (per second) at `t0` and `t1`.
    a: f64,
    b: f64,
    /// Iterations scheduled before `t0`.
    c0: f64,
}

impl Seg {
    fn area(&self) -> f64 {
        (self.a + self.b) / 2.0 * (self.t1 - self.t0)
    }
}

#[derive(Debug, Clone)]
pub struct ArrivalSchedule {
    segs: Vec<Seg>,
    idx: usize,
    k: u64,
}

impl ArrivalSchedule {
    /// `rate` iterations per second for `duration`.
    pub fn constant(rate: f64, duration: Duration) -> Self {
        Self::ramping(rate, &[(duration, rate)])
    }

    /// Starts at `start_rate` (per second) and ramps linearly to each stage's target rate.
    pub fn ramping(start_rate: f64, stages: &[(Duration, f64)]) -> Self {
        let mut segs = Vec::with_capacity(stages.len());
        let (mut t, mut rate, mut c) = (0.0f64, start_rate.max(0.0), 0.0f64);
        for (d, target) in stages {
            let target = target.max(0.0);
            let seg = Seg { t0: t, t1: t + d.as_secs_f64(), a: rate, b: target, c0: c };
            c += seg.area();
            t = seg.t1;
            rate = target;
            segs.push(seg);
        }
        Self { segs, idx: 0, k: 0 }
    }

    /// The schedule of an arrival-rate executor; `None` for VU executors.
    pub fn for_executor(ex: &Executor) -> Option<Self> {
        match ex {
            Executor::ConstantArrivalRate { rate, time_unit, duration, .. } => Some(Self::constant(per_sec(*rate, time_unit), duration.get())),
            Executor::RampingArrivalRate { start_rate, time_unit, stages, .. } => Some(Self::ramping(
                per_sec(*start_rate, time_unit),
                &stages.iter().map(|s| (s.duration.get(), per_sec(s.target, time_unit))).collect::<Vec<_>>(),
            )),
            _ => None,
        }
    }

    /// Total iterations the profile asks for (the integral of the rate).
    pub fn expected_iterations(&self) -> f64 {
        self.segs.last().map(|s| s.c0 + s.area()).unwrap_or(0.0)
    }

    pub fn total_duration(&self) -> Duration {
        Duration::from_secs_f64(self.segs.last().map(|s| s.t1).unwrap_or(0.0))
    }
}

impl Iterator for ArrivalSchedule {
    /// Intended start offset of the next iteration, from the start of the test.
    type Item = Duration;

    fn next(&mut self) -> Option<Duration> {
        let k = self.k as f64;
        while let Some(seg) = self.segs.get(self.idx) {
            let c1 = seg.c0 + seg.area();
            if seg.t1 > seg.t0 && k < c1 - 1e-9 {
                let delta = (k - seg.c0).max(0.0);
                let tau = if delta == 0.0 {
                    0.0
                } else {
                    // Solve c0 + a·τ + s·τ²/2 = k in a form that is stable as the slope → 0.
                    let s = (seg.b - seg.a) / (seg.t1 - seg.t0);
                    let disc = (seg.a * seg.a + 2.0 * s * delta).max(0.0);
                    let denom = seg.a + disc.sqrt();
                    if denom <= 0.0 { 0.0 } else { 2.0 * delta / denom }
                };
                self.k += 1;
                let t = (seg.t0 + tau).min(seg.t1);
                return Some(Duration::from_secs_f64(t));
            }
            self.idx += 1;
        }
        None
    }
}

/// VU target at `t` for a profile starting at `start` VUs and ramping linearly to each stage
/// target over the stage's duration. After the last stage, the last target holds.
pub fn vus_at(start: u32, stages: &[(Duration, u32)], t: Duration) -> u32 {
    let mut from = start as f64;
    let mut t0 = Duration::ZERO;
    for (d, target) in stages {
        let t1 = t0 + *d;
        if t < t1 {
            let frac = if d.is_zero() { 1.0 } else { (t - t0).as_secs_f64() / d.as_secs_f64() };
            return (from + (*target as f64 - from) * frac).round().max(0.0) as u32;
        }
        from = *target as f64;
        t0 = t1;
    }
    from as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(v: &[Duration]) -> Vec<f64> {
        v.iter().map(|d| (d.as_secs_f64() * 1000.0).round() / 1000.0).collect()
    }

    #[test]
    fn constant_rate_is_evenly_spaced() {
        let all: Vec<Duration> = ArrivalSchedule::constant(10.0, Duration::from_secs(1)).collect();
        assert_eq!(all.len(), 10);
        assert_eq!(secs(&all), vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]);
        assert_eq!(ArrivalSchedule::constant(1.0 / 3.0, Duration::from_secs(9)).count(), 3);
        assert_eq!(ArrivalSchedule::constant(250.0, Duration::from_secs(4)).count(), 1000);
    }

    #[test]
    fn ramp_up_matches_integral() {
        // 0 → 100/s over 2s (area 100), then 100/s for 1s (area 100).
        let s = ArrivalSchedule::ramping(0.0, &[(Duration::from_secs(2), 100.0), (Duration::from_secs(1), 100.0)]);
        assert!((s.expected_iterations() - 200.0).abs() < 1e-9);
        let all: Vec<Duration> = s.collect();
        assert_eq!(all.len(), 200);
        // Halfway through the ramp a quarter of its iterations have started (area grows with t²).
        let before_1s = all.iter().filter(|d| d.as_secs_f64() < 1.0).count();
        assert!((24..=26).contains(&before_1s), "{before_1s}");
        assert!(all.windows(2).all(|w| w[0] <= w[1]), "monotonic");
        assert!(all.last().unwrap().as_secs_f64() < 3.0);
    }

    #[test]
    fn ramp_down_and_zero_stages() {
        // Instant jump 100 → 50/s, then 50/s → 0 over 2s: area 50; most iterations happen early.
        let all: Vec<Duration> = ArrivalSchedule::ramping(100.0, &[(Duration::ZERO, 50.0), (Duration::from_secs(2), 0.0)]).collect();
        assert_eq!(all.len(), 50);
        let early = all.iter().filter(|d| d.as_secs_f64() < 1.0).count();
        assert!(early >= 37, "{early}");
        assert_eq!(ArrivalSchedule::ramping(0.0, &[(Duration::from_secs(5), 0.0)]).count(), 0);
    }

    #[test]
    fn vu_ramp_interpolates() {
        let stages = [(Duration::from_secs(10), 10), (Duration::from_secs(10), 10), (Duration::from_secs(10), 0)];
        assert_eq!(vus_at(0, &stages, Duration::ZERO), 0);
        assert_eq!(vus_at(0, &stages, Duration::from_secs(5)), 5);
        assert_eq!(vus_at(0, &stages, Duration::from_secs(15)), 10);
        assert_eq!(vus_at(0, &stages, Duration::from_secs(25)), 5);
        assert_eq!(vus_at(0, &stages, Duration::from_secs(99)), 0);
        assert_eq!(vus_at(4, &[(Duration::ZERO, 8)], Duration::ZERO), 8);
    }
}
