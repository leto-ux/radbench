//! Kernel log classification: which kernel messages indicate a
//! radiation-induced (or any) hardware/software fault worth alarming on.
//! Kept free of I/O so it can be unit-tested on the host.

/// Patterns (lower-cased substring match) and the category they map to.
/// First match wins, so more specific categories come first.
const PATTERNS: &[(&str, &str)] = &[
    ("kernel panic", "panic"),
    ("soft lockup", "lockup"),
    ("hard lockup", "lockup"),
    ("rcu_sched", "lockup"),
    ("rcu_preempt", "lockup"),
    ("rcu detected stall", "lockup"),
    ("hung_task", "lockup"),
    ("blocked for more than", "lockup"),
    ("unable to handle kernel", "oops"),
    ("internal error", "oops"),
    ("general protection", "oops"),
    ("oops", "oops"),
    ("bug:", "bug"),
    ("segfault", "segfault"),
    ("unhandled signal", "segfault"),
    ("illegal instruction", "illegal_insn"),
    ("undefined instruction", "illegal_insn"),
    ("edac", "edac"),
    ("ecc", "edac"),
    ("machine check", "mce"),
    ("ext4-fs error", "fs"),
    ("i/o error", "io"),
    ("mmc", "io"),
    ("corrupt", "corruption"),
    ("checksum", "corruption"),
    ("out of memory", "oom"),
    ("watchdog", "watchdog"),
    ("usb disconnect", "link"),
    ("link is down", "link"),
    ("thermal", "thermal"),
];

/// Benign lines that would otherwise match a pattern above.
const IGNORE: &[&str] = &["mmc0: new", "mmc1: new", "mmcblk0: p", "registered", "thermal_zone"];

/// Parse a `/dev/kmsg` record: `prio,seq,usec,flags;message`.
/// Returns (priority 0..7, message).
pub fn parse_kmsg(record: &str) -> Option<(u8, &str)> {
    let (hdr, msg) = record.split_once(';')?;
    let prio_fac: u32 = hdr.split(',').next()?.parse().ok()?;
    Some(((prio_fac & 7) as u8, msg.trim_end()))
}

/// Classify a kernel message. Returns a category if it looks like a fault.
/// Any message at priority ≤ 3 (err/crit/alert/emerg) is reported even if no
/// pattern matches, as "kernel_err".
pub fn classify(prio: u8, msg: &str) -> Option<&'static str> {
    let lower = msg.to_ascii_lowercase();
    if IGNORE.iter().any(|i| lower.contains(i)) {
        return None;
    }
    for (pat, cat) in PATTERNS {
        if lower.contains(pat) {
            return Some(cat);
        }
    }
    if prio <= 3 {
        return Some("kernel_err");
    }
    None
}

/// Name of a fatal signal, for crash reports.
pub fn signal_name(sig: i32) -> &'static str {
    match sig {
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        11 => "SIGSEGV",
        _ => "SIGNAL",
    }
}
