//! Performance measurements shown in the debug overlay.
//!
//! Phase 0 budgets (see PLAN.md): idle RAM < 150 MB, keypress -> frame on screen < 1 frame.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

const SAMPLES: usize = 120;

/// Measures the time from a keypress to the moment its frame is handed to the GPU for display.
///
/// This is the part of the latency the editor controls. The monitor adds its own scan-out
/// delay on top, which no program can avoid.
#[derive(Default)]
pub struct InputLatency {
    pending: Option<Instant>,
    samples: VecDeque<Duration>,
}

impl InputLatency {
    /// Call when an input event arrives. Only the oldest unanswered input is tracked, so a burst
    /// of keys measures the worst case.
    pub fn input(&mut self) {
        self.pending.get_or_insert_with(Instant::now);
    }

    /// Call right after a frame was presented.
    pub fn presented(&mut self) {
        if let Some(start) = self.pending.take() {
            if self.samples.len() == SAMPLES {
                self.samples.pop_front();
            }
            self.samples.push_back(start.elapsed());
        }
    }

    pub fn last(&self) -> Option<Duration> {
        self.samples.back().copied()
    }

    pub fn average(&self) -> Option<Duration> {
        let n = self.samples.len() as u32;
        (n > 0).then(|| self.samples.iter().sum::<Duration>() / n)
    }
}

/// Resident memory of this process in megabytes (working set on Windows).
pub fn ram_mb() -> Option<f64> {
    memory_stats::memory_stats().map(|m| m.physical_mem as f64 / (1024.0 * 1024.0))
}

pub fn ms(d: Option<Duration>) -> String {
    match d {
        Some(d) => format!("{:.1} ms", d.as_secs_f64() * 1000.0),
        None => "–".into(),
    }
}
