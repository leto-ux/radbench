use radbench::protocol::{Event, Packet, Status};
use radbench::reference;
use sha2::{Digest, Sha256};
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
    let uart_path = std::env::var("RADBENCH_UART").unwrap_or_else(|_| "/dev/ttyS0".into());
    let monitor_addr =
        std::env::var("RADBENCH_MONITOR").unwrap_or_else(|_| "192.168.1.100:9000".into());
    let log_path = std::env::var("RADBENCH_LOG").unwrap_or_else(|_| "/root/radbench.log".into());

    core_affinity::set_for_current(core_affinity::CoreId { id: cpu });

    let (tx, rx) = channel::<Packet>();
    let iter = Arc::new(AtomicU64::new(0));
    let iter_hb = Arc::clone(&iter);

    // Logger thread: local file + UART + TCP
    let logger = {
        let core = core.clone();
        let log_path = log_path.clone();
        thread::spawn(move || {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .unwrap();
            let mut uart = serialport::new(&uart_path, 115_200)
                .timeout(Duration::from_millis(100))
                .open()
                .ok();
            let mut tcp = TcpStream::connect(&monitor_addr).ok();
            let mut seq = 0u64;

            for pkt in rx {
                seq += 1;
                let mut pkt = pkt;
                pkt.seq = seq;
                let line = serde_json::to_string(&pkt).unwrap();

                // Local file with fsync
                let _ = writeln!(file, "{}", line);
                let _ = file.flush();
                unsafe {
                    libc::fsync(file.as_raw_fd());
                }

                // UART
                if let Some(u) = uart.as_mut() {
                    let _ = writeln!(u, "{}", line);
                }

                // TCP
                if let Some(t) = tcp.as_mut() {
                    let _ = writeln!(t, "{}", line);
                }

                // Periodic log integrity packet
                if seq % 500 == 0 {
                    let meta = file.metadata().unwrap();
                    let crc = crc32_file(&log_path);
                    let integrity = Packet {
                        seq: seq + 1,
                        ts: now_ms(),
                        source: format!("dut-{}", core),
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
                        let _ = writeln!(t, "{}", line);
                    }
                }
            }
        })
    };

    // Heartbeat thread
    let tx_hb = tx.clone();
    let core_hb = core.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(1));
            let _ = tx_hb.send(Packet {
                seq: 0,
                ts: now_ms(),
                source: format!("dut-{}", core_hb),
                event: Event::Heartbeat {
                    core: core_hb.clone(),
                    iter: iter_hb.load(Ordering::Relaxed),
                    temp_milli: read_temp(),
                },
            });
        }
    });

    // Memory test thread
    let tx_mem = tx.clone();
    let core_mem = core.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(10));
            if let Some(reason) = mem_march() {
                let _ = tx_mem.send(Packet {
                    seq: 0,
                    ts: now_ms(),
                    source: format!("dut-{}", core_mem),
                    event: Event::Error {
                        core: core_mem.clone(),
                        test: "mem_march".into(),
                        n: 0,
                        computed: None,
                        expected: None,
                        reason,
                    },
                });
            }
        }
    });

    // Run main workload
    run_fib(&core, &tx, &iter);

    // Graceful shutdown on SIGTERM/SIGINT would send Shutdown packet here
    logger.join().unwrap();
}

fn run_fib(core: &str, tx: &Sender<Packet>, iter: &AtomicU64) {
    let checkpoints = reference::checkpoints();
    let mut a: u128 = 0;
    let mut b: u128 = 1;
    let mut n = 0u64;
    let mut cp_idx = 0;

    loop {
        let c = a.wrapping_add(b);
        a = b;
        b = c;
        n += 1;
        iter.store(n, Ordering::Relaxed);

        if cp_idx < checkpoints.len() && n == checkpoints[cp_idx].n {
            let start = Instant::now();
            let exact = reference::fib(n);
            let mut h = Sha256::new();
            h.update(exact.to_bytes_be());
            let computed = hex::encode(h.finalize());
            let expected = &checkpoints[cp_idx].expected_hash;
            let status = if &computed == expected {
                Status::Ok
            } else {
                Status::Mismatch
            };
            let elapsed = start.elapsed().as_micros() as u64;

            tx.send(Packet {
                seq: 0,
                ts: now_ms(),
                source: format!("dut-{}", core),
                event: Event::Checkpoint {
                    core: core.to_string(),
                    test: "fib".into(),
                    n,
                    hash: computed.clone(),
                    status,
                    temp_milli: read_temp(),
                    elapsed_us: elapsed,
                },
            })
            .unwrap();

            if status == Status::Mismatch {
                tx.send(Packet {
                    seq: 0,
                    ts: now_ms(),
                    source: format!("dut-{}", core),
                    event: Event::Error {
                        core: core.to_string(),
                        test: "fib".into(),
                        n,
                        computed: Some(computed),
                        expected: Some(expected.clone()),
                        reason: "checkpoint hash mismatch".into(),
                    },
                })
                .unwrap();
            }

            cp_idx += 1;
            if cp_idx >= checkpoints.len() {
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
