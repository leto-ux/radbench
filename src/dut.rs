use radbench::fault::{FaultInjector, FaultMode, FaultTarget};
use radbench::protocol::{Event, Packet, Status};
use radbench::reference;
use radbench::workload::FibState;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::TcpStream;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn main() {
    let core = std::env::var("RADBENCH_CORE").unwrap_or_else(|_| "core0".into());
    let cpu: usize = std::env::var("RADBENCH_CPU")
        .unwrap_or_else(|_| "0".into())
        .parse()
        .unwrap();
    let uart_path = std::env::var("RADBENCH_UART").ok();
    let monitor_addr =
        std::env::var("RADBENCH_MONITOR").unwrap_or_else(|_| "192.168.1.100:9000".into());
    let log_path = std::env::var("RADBENCH_LOG").unwrap_or_else(|_| "/root/radbench.log".into());

    // Unique run_id so the monitor can distinguish restarts
    let run_id = format!("{:08x}", now_ms() & 0xFFFFFFFF);

    core_affinity::set_for_current(core_affinity::CoreId { id: cpu });

    let (tx, rx) = channel::<Packet>();
    let iter = Arc::new(AtomicU64::new(0));
    let epoch = Arc::new(AtomicU64::new(0));
    let iter_hb = Arc::clone(&iter);
    let epoch_hb = Arc::clone(&epoch);

    // Logger thread: local file + optional UART + reconnecting TCP
    let logger = {
        let core = core.clone();
        let log_path = log_path.clone();
        let monitor_addr = monitor_addr.clone();
        thread::spawn(move || {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .unwrap();

            // UART is optional — only open if RADBENCH_UART is set
            let mut uart = uart_path.as_ref().and_then(|p| {
                match serialport::new(p, 115_200)
                    .timeout(Duration::from_millis(100))
                    .open()
                {
                    Ok(port) => {
                        eprintln!("[logger] UART {} opened", p);
                        Some(port)
                    }
                    Err(e) => {
                        eprintln!("[logger] UART {} unavailable: {}", p, e);
                        None
                    }
                }
            });

            // TCP with reconnect
            let mut tcp: Option<TcpStream> = None;
            let mut tcp_backoff = Duration::from_secs(1);
            let mut tcp_last_attempt = Instant::now() - tcp_backoff;

            let mut seq = 0u64;

            for pkt in rx {
                seq += 1;
                let mut pkt = pkt;
                pkt.seq = seq;
                let line = serde_json::to_string(&pkt).unwrap();

                // local file with fsync
                let _ = writeln!(file, "{}", line);
                let _ = file.flush();
                unsafe {
                    libc::fsync(file.as_raw_fd());
                }

                // UART
                if let Some(u) = uart.as_mut() {
                    let _ = writeln!(u, "{}", line);
                }

                // TCP — reconnect if disconnected
                if tcp.is_none() && tcp_last_attempt.elapsed() >= tcp_backoff {
                    tcp_last_attempt = Instant::now();
                    match TcpStream::connect(&monitor_addr) {
                        Ok(s) => {
                            eprintln!("[logger] TCP connected to {}", monitor_addr);
                            tcp = Some(s);
                            tcp_backoff = Duration::from_secs(1);
                        }
                        Err(e) => {
                            eprintln!(
                                "[logger] TCP connect to {} failed: {} (retry in {:?})",
                                monitor_addr, e, tcp_backoff
                            );
                            tcp_backoff = (tcp_backoff * 2).min(Duration::from_secs(30));
                        }
                    }
                }
                if let Some(t) = tcp.as_mut() {
                    if writeln!(t, "{}", line).is_err() {
                        eprintln!("[logger] TCP write failed, will reconnect");
                        tcp = None;
                    }
                }

                // log integrity
                if seq % 500 == 0 {
                    let meta = file.metadata().unwrap();
                    let crc = crc32_file(&log_path);
                    let integrity = Packet {
                        seq: seq + 1,
                        ts: now_ms(),
                        source: format!("dut-{}", core),
                        run_id: None,
                        event: Event::LogIntegrity {
                            file: log_path.clone(),
                            bytes: meta.len(),
                            crc32: format!("{:08x}", crc),
                        },
                    };
                    let line = serde_json::to_string(&integrity).unwrap();
                    let _ = writeln!(file, "{}", line);
                    let _ = file.flush();
                    unsafe {
                        libc::fsync(file.as_raw_fd());
                    }
                    if let Some(u) = uart.as_mut() {
                        let _ = writeln!(u, "{}", line);
                    }
                    if let Some(t) = tcp.as_mut() {
                        if writeln!(t, "{}", line).is_err() {
                            eprintln!("[logger] TCP write failed, will reconnect");
                            tcp = None;
                        }
                    }
                }
            }
        })
    };

    // heartbeat
    let tx_hb = tx.clone();
    let core_hb = core.clone();
    let run_id_hb = run_id.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(1));
            let _ = tx_hb.send(Packet {
                seq: 0,
                ts: now_ms(),
                source: format!("dut-{}", core_hb),
                run_id: Some(run_id_hb.clone()),
                event: Event::Heartbeat {
                    core: core_hb.clone(),
                    epoch: epoch_hb.load(Ordering::Relaxed),
                    iter: iter_hb.load(Ordering::Relaxed),
                    temp_milli: read_temp(),
                },
            });
        }
    });

    // mem march
    let tx_mem = tx.clone();
    let core_mem = core.clone();
    let run_id_mem = run_id.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(10));
            if let Some(reason) = mem_march() {
                let _ = tx_mem.send(Packet {
                    seq: 0,
                    ts: now_ms(),
                    source: format!("dut-{}", core_mem),
                    run_id: Some(run_id_mem.clone()),
                    event: Event::Error {
                        core: core_mem.clone(),
                        test: "mem_march".into(),
                        epoch: 0,
                        n: 0,
                        computed: None,
                        expected: None,
                        reason,
                    },
                });
            }
        }
    });

    run_fib(&core, &tx, &iter, &epoch, &run_id);

    // graceful shutdown on SIGTERM/SIGINT would send sh packet here
    logger.join().unwrap();
}

fn run_fib(
    core: &str,
    tx: &Sender<Packet>,
    iter_counter: &AtomicU64,
    epoch_counter: &AtomicU64,
    run_id: &str,
) {
    let checkpoints = reference::checkpoints();
    let mut state = FibState::new();
    let mut cp_idx = 0;

    // Optional fault injection for testing (set RADBENCH_FAULT_RATE to enable)
    let mut fault_injector = std::env::var("RADBENCH_FAULT_RATE")
        .ok()
        .and_then(|rate_str| {
            let rate: f64 = rate_str.parse().ok()?;
            if rate <= 0.0 {
                return None;
            }
            let seed: u64 = std::env::var("RADBENCH_FAULT_SEED")
                .unwrap_or_else(|_| "42".into())
                .parse()
                .unwrap_or(42);
            let mode = match std::env::var("RADBENCH_FAULT_MODE")
                .unwrap_or_else(|_| "see".into())
                .as_str()
            {
                "tid" => {
                    let accel: f64 = std::env::var("RADBENCH_FAULT_ACCEL")
                        .unwrap_or_else(|_| "1.5".into())
                        .parse()
                        .unwrap_or(1.5);
                    FaultMode::Tid {
                        initial_rate: rate,
                        acceleration: accel,
                    }
                }
                _ => FaultMode::See { rate },
            };
            eprintln!("[dut] fault injection enabled: {:?}", mode);
            Some(FaultInjector::new(seed, mode))
        });

    loop {
        state.step();
        iter_counter.store(state.n, Ordering::Relaxed);

        // bit flips, disgusting code
        if let Some(ref mut fi) = fault_injector {
            if let Some(fault) = fi.maybe_fault() {
                match fault.target {
                    FaultTarget::FibA => state.flip_bit_a(fault.bit),
                    FaultTarget::FibB => state.flip_bit_b(fault.bit),
                }
            }
        }

        // core workload.rs loop
        if cp_idx < checkpoints.len() && state.n == checkpoints[cp_idx].n {
            let start = Instant::now();
            let result = state.check(&checkpoints[cp_idx]);
            let elapsed = start.elapsed().as_micros() as u64;

            let status = if result.passed {
                Status::Ok
            } else {
                Status::Mismatch
            };

            tx.send(Packet {
                seq: 0,
                ts: now_ms(),
                source: format!("dut-{}", core),
                run_id: Some(run_id.to_string()),
                event: Event::Checkpoint {
                    core: core.to_string(),
                    test: "fib".into(),
                    epoch: state.epoch,
                    n: state.n,
                    hash: result.hash.clone(),
                    status,
                    temp_milli: read_temp(),
                    elapsed_us: elapsed,
                },
            })
            .unwrap();

            if !result.passed {
                tx.send(Packet {
                    seq: 0,
                    ts: now_ms(),
                    source: format!("dut-{}", core),
                    run_id: Some(run_id.to_string()),
                    event: Event::Error {
                        core: core.to_string(),
                        test: "fib".into(),
                        epoch: state.epoch,
                        n: state.n,
                        computed: Some(result.hash),
                        expected: Some(result.expected.to_string()),
                        reason: "checkpoint hash mismatch".into(),
                    },
                })
                .unwrap();
            }

            cp_idx += 1;
            if cp_idx >= checkpoints.len() {
                // Epoch complete — reset for next cycle
                if let Some(ref mut fi) = fault_injector {
                    fi.on_epoch_end();
                }
                state.reset_epoch();
                epoch_counter.store(state.epoch, Ordering::Relaxed);
                cp_idx = 0;
            }
        }
    }
}

fn mem_march() -> Option<String> {
    let size = 1 << 20;
    let mut buf = vec![0u8; size];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (i & 0xFF) as u8;
    }
    for (i, b) in buf.iter().enumerate() {
        if *b != ((i & 0xFF) as u8) {
            return Some(format!("mem mismatch at offset {}", i));
        }
    }
    None
}

fn read_temp() -> Option<i32> {
    for z in 0..10 {
        let p = format!("/sys/class/thermal/thermal_zone{z}/temp");
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(v) = s.trim().parse::<i32>() {
                return Some(v);
            }
        }
    }
    None
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn crc32_file(path: &str) -> u32 {
    let data = std::fs::read(path).unwrap_or_default();
    let mut crc: u32 = 0xFFFFFFFF;
    for b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB88320
            } else {
                crc >> 1
            };
        }
    }
    crc ^ 0xFFFFFFFF
}
