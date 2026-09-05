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

        let dedup_key = (
            pkt.source.clone(),
            pkt.run_id.clone().unwrap_or_default(),
            pkt.seq,
        );
        if !seen.insert(dedup_key) {
            continue;
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

                // Create a per-DUT log file: logs/<timestamp>_<arch>_<core>.log
                let dut_log_path = format!(
                    "{}/{}_{}_{}_{}.log",
                    log_dir, session_ts, arch, core,
                    pkt.run_id.as_deref().unwrap_or("unknown")
                );
                let dut_log = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&dut_log_path)
                    .unwrap();
                eprintln!("[monitor] logging {} ({}) -> {}", core, arch, dut_log_path);

                // Write the announce packet to the DUT's log
                let line = serde_json::to_string(&pkt).unwrap();
                let mut state = DutState {
                    arch: arch.clone(),
                    log_file: dut_log,
                    _log_path: dut_log_path,
                };
                let _ = writeln!(state.log_file, "{}", line);
                let _ = state.log_file.flush();

                dut_states.insert(pkt.source.clone(), state);
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

                // Write to per-DUT log
                log_to_dut(&mut dut_states, &pkt);
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

                log_to_dut(&mut dut_states, &pkt);
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

                log_to_dut(&mut dut_states, &pkt);
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

                log_to_dut(&mut dut_states, &pkt);
            }
            _ => {
                log_to_dut(&mut dut_states, &pkt);
            }
        }
    }
}

/// Write a packet to the per-DUT log file (if the DUT has announced itself).
fn log_to_dut(dut_states: &mut HashMap<String, DutState>, pkt: &Packet) {
    if let Some(state) = dut_states.get_mut(&pkt.source) {
        let line = serde_json::to_string(pkt).unwrap();
        let _ = writeln!(state.log_file, "{}", line);
        let _ = state.log_file.flush();
    }
}

fn handle_stream(stream: TcpStream, tx: Sender<TaggedPacket>) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".into());
    eprintln!("[monitor] DUT connected from {}", peer);
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
