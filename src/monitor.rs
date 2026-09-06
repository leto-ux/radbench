use radbench::protocol::{Event, Packet, Status};
use radbench::reference;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::Duration;

/// Enriched packet: the original Packet plus the peer address (for routing to per-DUT logs).
struct TaggedPacket {
    pkt: Packet,
    peer: String,
}

/// Per-DUT state tracked by the monitor.
struct DutState {
    arch: String,
    log_file: std::fs::File,
    _log_path: String,
}

fn main() {
    // Support multiple listen addresses: comma-separated, e.g. "0.0.0.0:9000,0.0.0.0:9001"
    let listen_str =
        std::env::var("MONITOR_LISTEN").unwrap_or_else(|_| "0.0.0.0:9000".into());
    let listen_addrs: Vec<String> = listen_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let serial = std::env::var("MONITOR_SERIAL").ok();
    let log_dir = std::env::var("MONITOR_LOG_DIR").unwrap_or_else(|_| "logs".into());

    // Create log directory
    fs::create_dir_all(&log_dir).expect("failed to create log directory");

    println!("monitor self-test: verifying reference hashes...");
    let checkpoints = reference::checkpoints();
    for cp in checkpoints {
        let (a, b) = reference::fib_u128_at(cp.n);
        let recomputed = reference::hash_state(a, b);
        assert_eq!(
            recomputed, cp.expected_hash,
            "monitor self-test failed at n={}",
            cp.n
        );
    }
    println!(
        "monitor self-test passed ({} checkpoints)",
        checkpoints.len()
    );

    let session_ts = timestamp_now();

    let (tx, rx) = channel::<TaggedPacket>();

    // TCP — bind on each listen address
    for addr in &listen_addrs {
        let tx_tcp = tx.clone();
        let addr = addr.clone();
        thread::spawn(move || {
            let listener = TcpListener::bind(&addr).unwrap();
            println!("listening on {}", addr);
            for stream in listener.incoming() {
                if let Ok(s) = stream {
                    let tx = tx_tcp.clone();
                    thread::spawn(move || handle_stream(s, tx));
                }
            }
        });
    }

    // UART
    if let Some(path) = serial {
        let tx_serial = tx.clone();
        thread::spawn(move || {
            let port = serialport::new(&path, 115_200)
                .timeout(Duration::from_millis(100))
                .open()
                .expect("serial open failed");
            let reader = BufReader::new(port);
            for line in reader.lines() {
                if let Ok(l) = line {
                    if let Ok(p) = serde_json::from_str::<Packet>(&l) {
                        let _ = tx_serial.send(TaggedPacket {
                            pkt: p,
                            peer: "serial".into(),
                        });
                    }
                }
            }
        });
    }

    let mut seen = HashSet::new();
    let mut last_heartbeat: HashMap<String, u64> = HashMap::new();

    // Session-level alarm log (timestamped, never overwritten)
    let alarm_path = format!("{}/alarms_{}.log", log_dir, session_ts);
    let mut alarm = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&alarm_path)
        .unwrap();
    eprintln!("[monitor] alarm log: {}", alarm_path);

    // Per-DUT state: keyed by source name (e.g. "dut-arm", "dut-riscv")
    let mut dut_states: HashMap<String, DutState> = HashMap::new();

    for tagged in rx {
        let pkt = tagged.pkt;
        let peer = tagged.peer;

        // Only deduplicate packets with non-zero sequence numbers so re-announce packets on reconnect are processed
        if pkt.seq != 0 {
            let dedup_key = (
                pkt.source.clone(),
                pkt.run_id.clone().unwrap_or_default(),
                pkt.seq,
            );
            if !seen.insert(dedup_key) {
                continue;
            }
        }

        let run_tag = pkt
            .run_id
            .as_deref()
            .map(|r| format!(" run={}", r))
            .unwrap_or_default();

        match &pkt.event {
            Event::Announce { core, arch, uname } => {
                eprintln!(
                    "[monitor] DUT announced: core={} arch={} peer={} uname={}",
                    core, arch, peer, uname
                );
            }
            Event::Checkpoint {
                core,
                test,
                epoch,
                n,
                hash,
                status,
                elapsed_us,
                ..
            } => {
                let expected = checkpoints
                    .iter()
                    .find(|c| c.n == *n)
                    .map(|c| c.expected_hash);
                let ok = expected.map(|e| e == hash.as_str()).unwrap_or(false);

                let arch_tag = dut_states
                    .get(&pkt.source)
                    .map(|s| format!("[{}]", s.arch))
                    .unwrap_or_default();

                if ok && *status == Status::Ok {
                    eprintln!(
                        "checkpoint{} {}{} epoch={} n={} OK ({}µs)",
                        run_tag, core, arch_tag, epoch, n, elapsed_us
                    );
                } else {
                    let msg = format!(
                        "ALARM ts={}{} {}{} core={} test={} epoch={} n={} expected={:?} got={}",
                        pkt.ts, run_tag, core, arch_tag, core, test, epoch, n, expected, hash
                    );
                    eprintln!("{}", msg);
                    writeln!(alarm, "{}", msg).unwrap();
                    alarm.flush().unwrap();
                }
            }
            Event::Heartbeat {
                core, epoch, iter, ..
            } => {
                let arch_tag = dut_states
                    .get(&pkt.source)
                    .map(|s| format!("[{}]", s.arch))
                    .unwrap_or_default();

                last_heartbeat.insert(core.clone(), pkt.ts);
                eprintln!(
                    "heartbeat{} {}{} epoch={} iter={}",
                    run_tag, core, arch_tag, epoch, iter
                );
            }
            Event::Error { .. } => {
                let arch_tag = dut_states
                    .get(&pkt.source)
                    .map(|s| format!("[{}]", s.arch))
                    .unwrap_or_default();

                let msg = format!(
                    "ALARM ts={}{} {}{} DUT-ERROR {:?}",
                    pkt.ts, run_tag, pkt.source, arch_tag, pkt
                );
                eprintln!("{}", msg);
                writeln!(alarm, "{}", msg).unwrap();
                alarm.flush().unwrap();
            }
            Event::Shutdown {
                core,
                reason,
                final_iter,
            } => {
                let arch_tag = dut_states
                    .get(&pkt.source)
                    .map(|s| format!("[{}]", s.arch))
                    .unwrap_or_default();

                let msg = format!(
                    "SHUTDOWN ts={}{} core={}{} reason={} final_iter={}",
                    pkt.ts, run_tag, core, arch_tag, reason, final_iter
                );
                eprintln!("{}", msg);
                writeln!(alarm, "{}", msg).unwrap();
                alarm.flush().unwrap();
            }
            _ => {}
        }

        // Always write every packet to the per-DUT log file (resumes or creates log file)
        log_to_dut(&mut dut_states, &log_dir, &session_ts, &pkt);
    }
}

fn ensure_dut_state<'a>(
    dut_states: &'a mut HashMap<String, DutState>,
    log_dir: &str,
    session_ts: &str,
    pkt: &Packet,
    arch_hint: Option<&str>,
    core_hint: Option<&str>,
) -> &'a mut DutState {
    if !dut_states.contains_key(&pkt.source) {
        let run_id = pkt.run_id.as_deref().unwrap_or("unknown");
        let core = core_hint
            .or_else(|| pkt.source.strip_prefix("dut-"))
            .unwrap_or("unknown");
        let arch = arch_hint.unwrap_or("unknown");

        // Check if an existing log file for this run_id exists in log_dir to resume it
        let mut target_log_path = None;
        if run_id != "unknown" {
            if let Ok(entries) = fs::read_dir(log_dir) {
                let suffix = format!("_{}.log", run_id);
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(&suffix) && !name.starts_with("alarms_") {
                        target_log_path = Some(format!("{}/{}", log_dir, name));
                        break;
                    }
                }
            }
        }

        let (log_path, resumed) = match target_log_path {
            Some(p) => (p, true),
            None => (
                format!("{}/{}_{}_{}_{}.log", log_dir, session_ts, arch, core, run_id),
                false,
            ),
        };

        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap_or_else(|e| panic!("failed to open dut log {}: {}", log_path, e));

        if resumed {
            eprintln!(
                "[monitor] resumed logging {} ({}) -> {}",
                core, arch, log_path
            );
        } else {
            eprintln!("[monitor] logging {} ({}) -> {}", core, arch, log_path);
        }

        dut_states.insert(
            pkt.source.clone(),
            DutState {
                arch: arch.to_string(),
                log_file,
                _log_path: log_path,
            },
        );
    }
    dut_states.get_mut(&pkt.source).unwrap()
}

/// Write a packet to the per-DUT log file (resumes or creates log file if needed).
fn log_to_dut(
    dut_states: &mut HashMap<String, DutState>,
    log_dir: &str,
    session_ts: &str,
    pkt: &Packet,
) {
    let core_hint = match &pkt.event {
        Event::Checkpoint { core, .. }
        | Event::Heartbeat { core, .. }
        | Event::Error { core, .. }
        | Event::Shutdown { core, .. }
        | Event::Announce { core, .. } => Some(core.as_str()),
        _ => None,
    };
    let arch_hint = match &pkt.event {
        Event::Announce { arch, .. } => Some(arch.as_str()),
        _ => None,
    };

    let state = ensure_dut_state(dut_states, log_dir, session_ts, pkt, arch_hint, core_hint);
    if let Some(arch) = arch_hint {
        if state.arch == "unknown" {
            state.arch = arch.to_string();
        }
    }
    let line = serde_json::to_string(pkt).unwrap();
    let _ = writeln!(state.log_file, "{}", line);
    let _ = state.log_file.flush();
}

fn handle_stream(stream: TcpStream, tx: Sender<TaggedPacket>) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".into());
    eprintln!("[monitor] DUT connected from {}", peer);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        if let Ok(l) = line {
            if let Ok(p) = serde_json::from_str::<Packet>(&l) {
                let _ = tx.send(TaggedPacket {
                    pkt: p,
                    peer: peer.clone(),
                });
            }
        }
    }
    eprintln!("[monitor] DUT disconnected: {}", peer);
}

/// Generate a filesystem-safe timestamp for log filenames.
fn timestamp_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Convert to a readable timestamp (UTC)
    let s = secs;
    let days = s / 86400;
    let time_of_day = s % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Simple days-since-epoch to Y-M-D (good enough for filenames)
    let (year, month, day) = days_to_ymd(days);

    format!(
        "{:04}{:02}{:02}_{:02}{:02}{:02}",
        year, month, day, hours, minutes, seconds
    )
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Days since 1970-01-01
    let mut year = 1970u64;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let months = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1u64;
    for m in months {
        if days < m {
            break;
        }
        days -= m;
        month += 1;
    }
    (year, month, days + 1)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
