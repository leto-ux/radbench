//! Monitor-side DUT liveness tracking.
//!
//! A crash, hang, kernel panic or reboot produces *no* error packet — the DUT
//! simply goes quiet. This module turns that silence (and other anomalies in
//! the heartbeat stream) into explicit alarms.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum LivenessAlarm {
    /// No heartbeat for `silent_ms` (first alarm per outage).
    Silent { source: String, silent_ms: u64 },
    /// Heartbeats resumed after an outage.
    Recovered { source: String, outage_ms: u64 },
    /// Heartbeats arrive but the workload iteration counter stopped moving.
    Stalled { source: String, iter: u64, for_ms: u64 },
    /// Iteration rate dropped well below the running baseline.
    Slow { source: String, rate: f64, baseline: f64 },
    /// Same source came back with a different run_id (process/board restart).
    Restart { source: String, old_run: String, new_run: String },
}

#[derive(Debug, Clone, Copy)]
pub struct LivenessConfig {
    pub timeout_ms: u64,
    pub stall_ms: u64,
    /// Alarm when rate < baseline × slow_ratio.
    pub slow_ratio: f64,
    /// EWMA smoothing for the baseline rate.
    pub ewma_alpha: f64,
}

impl Default for LivenessConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 5_000,
            stall_ms: 5_000,
            slow_ratio: 0.5,
            ewma_alpha: 0.05,
        }
    }
}

#[derive(Debug, Default)]
struct DutLive {
    run_id: String,
    /// Monitor-clock time of the last heartbeat.
    last_seen: u64,
    silent_alarmed: bool,
    /// (epoch, iter) progress tracking.
    last_progress: Option<(u64, u64)>,
    last_progress_at: u64,
    stall_alarmed: bool,
    prev_hb: Option<(u64, u64, u64)>, // (ts, epoch, iter)
    baseline: Option<f64>,
    slow_alarmed: bool,
}

#[derive(Default)]
pub struct Liveness {
    cfg: LivenessConfig,
    duts: HashMap<String, DutLive>,
}

impl Liveness {
    pub fn new(cfg: LivenessConfig) -> Self {
        Self {
            cfg,
            duts: HashMap::new(),
        }
    }

    /// Record any packet from `source` (all packets prove the link is alive,
    /// but only heartbeats carry progress). `now` is the monitor clock.
    pub fn on_packet(&mut self, source: &str, run_id: Option<&str>, now: u64) -> Vec<LivenessAlarm> {
        let mut out = vec![];
        let d = self.duts.entry(source.to_string()).or_insert_with(|| DutLive {
            last_seen: now,
            last_progress_at: now,
            ..Default::default()
        });
        if let Some(r) = run_id {
            if d.run_id.is_empty() {
                d.run_id = r.to_string();
            } else if d.run_id != r {
                out.push(LivenessAlarm::Restart {
                    source: source.to_string(),
                    old_run: std::mem::replace(&mut d.run_id, r.to_string()),
                    new_run: r.to_string(),
                });
                // Fresh process: forget progress/rate history.
                d.last_progress = None;
                d.prev_hb = None;
                d.baseline = None;
                d.stall_alarmed = false;
                d.slow_alarmed = false;
                d.last_progress_at = now;
            }
        }
        if d.silent_alarmed {
            out.push(LivenessAlarm::Recovered {
                source: source.to_string(),
                outage_ms: now.saturating_sub(d.last_seen),
            });
            d.silent_alarmed = false;
            // The outage was already reported; give progress a fresh window
            // instead of immediately calling it a stall.
            d.last_progress_at = now;
            d.stall_alarmed = false;
        }
        d.last_seen = now;
        out
    }

    /// Record a heartbeat's progress counters. `ts` is the DUT timestamp
    /// (used for rate, so network jitter doesn't skew it).
    pub fn on_heartbeat(&mut self, source: &str, ts: u64, epoch: u64, iter: u64, now: u64) -> Vec<LivenessAlarm> {
        let cfg = self.cfg;
        let mut out = vec![];
        let Some(d) = self.duts.get_mut(source) else {
            return out;
        };

        if d.last_progress != Some((epoch, iter)) {
            d.last_progress = Some((epoch, iter));
            d.last_progress_at = now;
            d.stall_alarmed = false;
        }

        // Rate is only meaningful within an epoch (iter resets on rollover).
        if let Some((pts, pep, pit)) = d.prev_hb {
            let dt = ts.saturating_sub(pts);
            if pep == epoch && iter >= pit && dt > 0 {
                let rate = (iter - pit) as f64 * 1000.0 / dt as f64;
                match d.baseline {
                    None => d.baseline = Some(rate),
                    Some(b) => {
                        if rate < b * cfg.slow_ratio {
                            if !d.slow_alarmed && rate > 0.0 {
                                out.push(LivenessAlarm::Slow {
                                    source: source.to_string(),
                                    rate,
                                    baseline: b,
                                });
                            }
                            d.slow_alarmed = true;
                        } else {
                            d.slow_alarmed = false;
                            // Only learn from healthy samples.
                            d.baseline = Some(b + cfg.ewma_alpha * (rate - b));
                        }
                    }
                }
            }
        }
        d.prev_hb = Some((ts, epoch, iter));
        out
    }

    /// Periodic check for silence and stalls.
    pub fn tick(&mut self, now: u64) -> Vec<LivenessAlarm> {
        let cfg = self.cfg;
        let mut out = vec![];
        for (src, d) in self.duts.iter_mut() {
            let silent = now.saturating_sub(d.last_seen);
            if silent >= cfg.timeout_ms && !d.silent_alarmed {
                d.silent_alarmed = true;
                out.push(LivenessAlarm::Silent {
                    source: src.clone(),
                    silent_ms: silent,
                });
            }
            let stuck = now.saturating_sub(d.last_progress_at);
            if !d.silent_alarmed && d.last_progress.is_some() && stuck >= cfg.stall_ms && !d.stall_alarmed {
                d.stall_alarmed = true;
                out.push(LivenessAlarm::Stalled {
                    source: src.clone(),
                    iter: d.last_progress.map(|p| p.1).unwrap_or(0),
                    for_ms: stuck,
                });
            }
        }
        out
    }
}
