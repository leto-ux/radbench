use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Packet {
    pub seq: u64,
    pub ts: u64,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub event: Event,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    #[serde(rename = "checkpoint")]
    Checkpoint {
        core: String,
        test: String,
        epoch: u64,
        n: u64,
        hash: String,
        status: Status,
        temp_milli: Option<i32>,
        elapsed_us: u64,
    },
    #[serde(rename = "heartbeat")]
    Heartbeat {
        core: String,
        epoch: u64,
        iter: u64,
        temp_milli: Option<i32>,
    },
    #[serde(rename = "error")]
    Error {
        core: String,
        test: String,
        epoch: u64,
        n: u64,
        computed: Option<String>,
        expected: Option<String>,
        reason: String,
    },
    #[serde(rename = "log_integrity")]
    LogIntegrity {
        file: String,
        bytes: u64,
        crc32: String,
    },
    #[serde(rename = "shutdown")]
    Shutdown {
        core: String,
        reason: String,
        final_iter: u64,
    },
    /// A single corrupted word found by a memory scrub.
    #[serde(rename = "upset")]
    Upset {
        core: String,
        /// Which region: "mem" (DRAM-resident) or "cache" (L1-resident).
        test: String,
        scrub: u64,
        offset: u64,
        expected: String,
        actual: String,
        flipped_bits: u32,
        /// Same word was also wrong in the previous scrub (hard/stuck fault).
        persistent: bool,
    },
    /// Summary of one or more scrub passes. `bytes × interval` is the
    /// exposure needed to turn upset counts into a cross-section.
    #[serde(rename = "scrub")]
    Scrub {
        core: String,
        test: String,
        bytes: u64,
        /// Passes aggregated into this summary.
        passes: u64,
        bad_words: u64,
        bad_bits: u64,
        /// Wall time spent reading for the last pass.
        duration_us: u64,
    },
    #[serde(rename = "announce")]
    Announce {
        core: String,
        arch: String,
        uname: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Status {
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "mismatch")]
    Mismatch,
    #[serde(rename = "timeout")]
    Timeout,
}
