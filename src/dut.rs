use radbench::fault::{FaultInjector, FaultMode, FaultTarget};
use radbench::lanes;
use radbench::memtest::MemRegion;
use radbench::regs::RegWorkload;
use radbench::protocol::{Event, Packet, Status};
use radbench::reference;
use radbench::workload::FibState;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::os::unix::io::AsRawFd;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Global flag: SIGUSR1 or UDP "flip" sets this, fib loop consumes it
static INJECT_PENDING: AtomicBool = AtomicBool::new(false);
// UDP "flipreg" sets this, register workload consumes it
static REG_INJECT_PENDING: AtomicBool = AtomicBool::new(false);
// UDP "flipmem" sets this, the DRAM scrubber consumes it before its next pass
static MEM_INJECT_PENDING: AtomicBool = AtomicBool::new(false);

/// fd of the crash-marker file, written from fatal-signal handlers.
static CRASH_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn handle_fatal(sig: libc::c_int) {
    let name = radbench::kmsg::signal_name(sig);
    let fd = CRASH_FD.load(Ordering::Relaxed);
    // Only async-signal-safe calls here: write(2) and fsync(2).
    unsafe {
        if fd >= 0 {
            libc::write(fd, name.as_ptr() as *const libc::c_void, name.len());
            libc::fsync(fd);
        }
        let msg = b"[dut] fatal signal, see crash marker\n";
        libc::write(2, msg.as_ptr() as *const libc::c_void, msg.len());
    }
    // SA_RESETHAND restored the default action; returning re-executes the
    // faulting instruction (or we re-raise for async signals) and the process
    // dies normally, so the run script restarts it.
    unsafe {
        libc::raise(sig);
    }
}

/// Record panics and fatal signals into `<log>.crash` so the *next* run can
/// report why the previous one died (the dying process may not get a packet
/// out). A panic in any thread aborts the process: a half-dead DUT (e.g. a
/// dead heartbeat thread) is harder to interpret than a clean restart.
fn install_crash_handlers(marker: &str) {
    if let Ok(f) = OpenOptions::new().create(true).append(true).open(marker) {
        CRASH_FD.store(f.as_raw_fd(), Ordering::Relaxed);
        std::mem::forget(f);
    }
    let marker = marker.to_string();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("[dut] panic: {}", info);
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&marker) {
            let _ = write!(f, "panic: {}", info);
            let _ = f.sync_all();
        }
        std::process::abort();
    }));
    for sig in [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE, libc::SIGABRT] {
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = handle_fatal as *const () as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESETHAND | libc::SA_ONSTACK;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

/// Findings about how the previous run / boot ended, reported at startup as
/// (test, reason) pairs.
fn previous_run_report(log_path: &str) -> Vec<(String, String)> {
    let mut out = vec![];

    // Process crash marker from the previous run.
    let marker = format!("{}.crash", log_path);
    if let Ok(s) = std::fs::read_to_string(&marker) {
        if !s.trim().is_empty() {
            let when = std::fs::metadata(&marker)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis().to_string())
                .unwrap_or_else(|| "?".into());
            out.push(("crash".into(), format!("previous run died: {} (at ts={})", s.trim(), when)));
        }
        let _ = std::fs::write(&marker, b"");
    }

    // Board reboot detection via the kernel boot id.
    let boot_id_path = format!("{}.bootid", log_path);
    if let Ok(cur) = std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        let cur = cur.trim();
        if let Ok(prev) = std::fs::read_to_string(&boot_id_path) {
            if prev.trim() != cur {
                let uptime = std::fs::read_to_string("/proc/uptime")
                    .ok()
                    .and_then(|u| u.split_whitespace().next().map(str::to_string))
                    .unwrap_or_else(|| "?".into());
                out.push(("reboot".into(), format!("board rebooted since previous run (uptime {} s)", uptime)));
            }
        }
        let _ = std::fs::write(&boot_id_path, cur);
    }

    // Kernel crash records preserved across reboot (if pstore/ramoops is set up).
    if let Ok(entries) = std::fs::read_dir("/sys/fs/pstore") {
        for e in entries.flatten() {
            let path = e.path();
            let data = std::fs::read(&path).unwrap_or_default();
            let tail = &data[data.len().saturating_sub(2048)..];
            out.push((
                "pstore".into(),
                format!("{}: {}", path.display(), String::from_utf8_lossy(tail)),
            ));
            // Content is now in our log; remove so it's not re-reported.
            let _ = std::fs::remove_file(&path);
        }
    }
    out
}

/// Follow /dev/kmsg and report fault-looking kernel messages.
fn spawn_kmsg_watcher(tx: &Sender<Packet>, core: &str, run_id: &str) {
    let fd = unsafe { libc::open(c"/dev/kmsg".as_ptr(), libc::O_RDONLY) };
    if fd < 0 {
        eprintln!("[dut] /dev/kmsg unavailable, kernel fault watch disabled");
        return;
    }
    // Skip history; previous-boot problems are covered by pstore/boot id.
    unsafe { libc::lseek(fd, 0, libc::SEEK_END) };
    eprintln!("[dut] watching kernel log");
    let (tx, core, run_id) = (tx.clone(), core.to_string(), run_id.to_string());
    thread::spawn(move || {
        let mut buf = vec![0u8; 8192];
        let mut window_start = Instant::now();
        let mut sent = 0u32;
        let mut suppressed = 0u32;
        loop {
            let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EPIPE) {
                    continue; // ring buffer overwrote unread records
                }
                thread::sleep(Duration::from_secs(1));
                continue;
            }
            let rec = String::from_utf8_lossy(&buf[..n as usize]).to_string();
            let Some((prio, msg)) = radbench::kmsg::parse_kmsg(&rec) else { continue };
            let Some(cat) = radbench::kmsg::classify(prio, msg) else { continue };
            // At most 30 reports/minute so a message storm can't flood the link.
            if window_start.elapsed() > Duration::from_secs(60) {
                if suppressed > 0 {
                    eprintln!("[dut] {} kernel messages suppressed", suppressed);
                }
                window_start = Instant::now();
                sent = 0;
                suppressed = 0;
            }
            if sent >= 30 {
                suppressed += 1;
                continue;
            }
            sent += 1;
            let _ = tx.send(Packet {
                seq: 0,
                ts: now_ms(),
                source: format!("dut-{}", core),
                run_id: Some(run_id.clone()),
                event: Event::Error {
                    core: core.clone(),
                    test: format!("kernel:{}", cat),
                    epoch: 0,
                    n: 0,
                    computed: None,
                    expected: None,
                    reason: format!("{}{}", msg, if suppressed > 0 { format!(" (+{} suppressed)", suppressed) } else { String::new() }),
                },
            });
        }
    });
}

extern "C" fn handle_sigusr1(_sig: libc::c_int) {
    INJECT_PENDING.store(true, Ordering::SeqCst);
}

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

    let previous = previous_run_report(&log_path);
    install_crash_handlers(&format!("{}.crash", log_path));

    let arch = detect_arch();
    let uname_full = detect_uname();
    eprintln!("[dut] architecture: {} ({})", arch, uname_full);

    core_affinity::set_for_current(core_affinity::CoreId { id: cpu });

    let (tx, rx) = channel::<Packet>();
    let iter = Arc::new(AtomicU64::new(0));
    let epoch = Arc::new(AtomicU64::new(0));
    let iter_hb = Arc::clone(&iter);
    let epoch_hb = Arc::clone(&epoch);

    // Logger thread: local file + optional UART + reconnecting TCP
    let logger = {
        let core = core.clone();
        let arch = arch.clone();
        let uname_full = uname_full.clone();
        let run_id = run_id.clone();
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
                    let connect_res = if let Ok(mut addrs) = monitor_addr.to_socket_addrs() {
                        if let Some(addr) = addrs.next() {
                            TcpStream::connect_timeout(&addr, Duration::from_secs(3))
                        } else {
                            TcpStream::connect(&monitor_addr)
                        }
                    } else {
                        TcpStream::connect(&monitor_addr)
                    };

                    match connect_res {
                        Ok(mut s) => {
                            let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
                            let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
                            eprintln!("[logger] TCP connected to {}", monitor_addr);

                            // Send announce packet on connection/reconnection
                            let announce = Packet {
                                seq: 0,
                                ts: now_ms(),
                                source: format!("dut-{}", core),
                                run_id: Some(run_id.clone()),
                                event: Event::Announce {
                                    core: core.clone(),
                                    arch: arch.clone(),
                                    uname: uname_full.clone(),
                                },
                            };
                            if let Ok(ann_line) = serde_json::to_string(&announce) {
                                let _ = writeln!(s, "{}", ann_line);
                            }

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
                        tcp_last_attempt = Instant::now();
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
                            tcp_last_attempt = Instant::now();
                        }
                    }
                }
            }
        })
    };

    // Send initial announce packet so local log & UART also record it
    let _ = tx.send(Packet {
        seq: 0,
        ts: now_ms(),
        source: format!("dut-{}", core),
        run_id: Some(run_id.clone()),
        event: Event::Announce {
            core: core.clone(),
            arch: arch.clone(),
            uname: uname_full,
        },
    });

    for (test, reason) in previous {
        eprintln!("[dut] {}: {}", test, reason);
        let _ = tx.send(Packet {
            seq: 0,
            ts: now_ms(),
            source: format!("dut-{}", core),
            run_id: Some(run_id.clone()),
            event: Event::Error {
                core: core.clone(),
                test,
                epoch: 0,
                n: 0,
                computed: None,
                expected: None,
                reason,
            },
        });
    }

    spawn_kmsg_watcher(&tx, &core, &run_id);

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

    // memory upset scrubbers (large DRAM region + small cache-resident region)
    spawn_scrubbers(&tx, &core, &run_id, cpu);

    // register-file / datapath workload
    if env_u64("RADBENCH_REGS", 1) != 0 {
        let (tx, core, run_id) = (tx.clone(), core.clone(), run_id.clone());
        thread::spawn(move || run_regs(&core, &tx, &run_id, cpu));
    }

    // Register SIGUSR1 → inject a single fault
    unsafe {
        libc::signal(libc::SIGUSR1, handle_sigusr1 as *const () as libc::sighandler_t);
    }
    eprintln!("[dut] SIGUSR1 registered for fault injection");

    // UDP control channel (default port 9001, separate from TCP monitor)
    let control_addr = std::env::var("RADBENCH_CONTROL")
        .unwrap_or_else(|_| "0.0.0.0:9001".into());
    match UdpSocket::bind(&control_addr) {
        Ok(sock) => {
            eprintln!("[dut] control channel on udp://{}", control_addr);
            thread::spawn(move || {
                let mut buf = [0u8; 256];
                loop {
                    if let Ok((len, src)) = sock.recv_from(&mut buf) {
                        let cmd = std::str::from_utf8(&buf[..len])
                            .unwrap_or("")
                            .trim();
                        match cmd {
                            "flip" => {
                                INJECT_PENDING.store(true, Ordering::SeqCst);
                                eprintln!("[control] fault requested from {}", src);
                            }
                            "flipreg" => {
                                REG_INJECT_PENDING.store(true, Ordering::SeqCst);
                                eprintln!("[control] register fault requested from {}", src);
                            }
                            "flipmem" => {
                                MEM_INJECT_PENDING.store(true, Ordering::SeqCst);
                                eprintln!("[control] memory fault requested from {}", src);
                            }
                            "status" => {
                                let _ = sock.send_to(b"ok\n", src);
                            }
                            other => {
                                eprintln!("[control] unknown command from {}: {:?}", src, other);
                            }
                        }
                    }
                }
            });
        }
        Err(e) => {
            eprintln!("[dut] control channel bind failed: {} (continuing without)", e);
        }
    }

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

        // On-demand fault injection (SIGUSR1 or UDP "flip")
        if INJECT_PENDING
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
        {
            let bit = (state.n % 128) as u8;
            state.flip_bit_a(bit);
            eprintln!(
                "[dut] synthetic fault injected: a bit {} at iter {} epoch {}",
                bit, state.n, state.epoch
            );
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
                    computed: None,
                    expected: None,
                    reason: format!("synthetic_fault: flipped a[{}]", bit),
                },
            })
            .unwrap();
        }

        // Continuous fault injection (env-var configured, for automated testing)
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
                    passes: None,
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

fn run_regs(core: &str, tx: &Sender<Packet>, run_id: &str, cpu: usize) {
    core_affinity::set_for_current(core_affinity::CoreId { id: cpu });
    eprintln!(
        "[dut] register workload: {} int + {} fp lanes",
        lanes::INT_LANES,
        lanes::FP_LANES
    );
    let mk = |event| Packet {
        seq: 0,
        ts: now_ms(),
        source: format!("dut-{}", core),
        run_id: Some(run_id.to_string()),
        event,
    };
    let mut w = RegWorkload::new();
    let mut last_ok_report = Instant::now() - Duration::from_secs(10);
    let mut passes_since_report = 0u64;
    loop {
        for idx in 0..lanes::CHECKS_PER_EPOCH {
            let start = Instant::now();
            w.advance(lanes::STEPS_PER_CHECK);
            if REG_INJECT_PENDING.swap(false, Ordering::SeqCst) {
                let lane = (w.n as usize / 7) % lanes::INT_LANES;
                w.flip_int(lane, (w.n % 64) as u32);
                eprintln!("[dut] synthetic register fault injected in int lane {}", lane);
            }
            let res = w.check(idx);
            let elapsed = start.elapsed().as_micros() as u64;
            passes_since_report += 1;
            // OK checks are rate-limited to ~1/s (each packet is fsynced);
            // mismatches are always reported.
            if res.passed() && last_ok_report.elapsed() < Duration::from_secs(1) {
                continue;
            }
            last_ok_report = Instant::now();
            let fold = w.int.iter().fold(0u64, |a, x| a.rotate_left(7) ^ x);
            let _ = tx.send(mk(Event::Checkpoint {
                core: core.to_string(),
                test: "regs".into(),
                epoch: w.epoch,
                n: w.n,
                hash: format!("{:016x}", fold),
                status: if res.passed() { Status::Ok } else { Status::Mismatch },
                temp_milli: read_temp(),
                elapsed_us: elapsed,
                passes: Some(std::mem::take(&mut passes_since_report)),
            }));
            if !res.passed() {
                let detail: Vec<String> = res
                    .int_bad
                    .iter()
                    .map(|(l, x)| format!("int[{}]^{:016x}", l, x))
                    .chain(res.fp_bad.iter().map(|(l, x)| format!("fp[{}]^{:016x}", l, x)))
                    .collect();
                let _ = tx.send(mk(Event::Error {
                    core: core.to_string(),
                    test: "regs".into(),
                    epoch: w.epoch,
                    n: w.n,
                    computed: None,
                    expected: None,
                    reason: format!(
                        "register lane mismatch ({} bits): {}",
                        res.flipped_bits(),
                        detail.join(" ")
                    ),
                }));
                // Resync so one upset isn't re-reported at every later check.
                w.reset_epoch();
                break;
            }
        }
        if w.n == lanes::STEPS_PER_CHECK * lanes::CHECKS_PER_EPOCH as u64 {
            w.reset_epoch();
        }
    }
}

/// Size of the DRAM test region: RADBENCH_MEM_MB, or by default half of
/// MemAvailable so as many cells as possible are under test.
fn mem_region_bytes() -> usize {
    if let Some(mb) = std::env::var("RADBENCH_MEM_MB")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        return mb << 20;
    }
    let avail_kb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemAvailable:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<usize>().ok())
        })
        .unwrap_or(16 << 10);
    ((avail_kb << 10) / 2).clamp(1 << 20, 1 << 30)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Run one scrub and forward its upsets + summary to the logger.
fn scrub_and_report(
    region: &mut MemRegion,
    test: &str,
    tx: &Sender<Packet>,
    core: &str,
    run_id: &str,
    agg: &mut (u64, u64, u64),
    summary_every: u64,
) {
    let start = Instant::now();
    let rep = region.scrub();
    let dur = start.elapsed().as_micros() as u64;
    let mk = |event| Packet {
        seq: 0,
        ts: now_ms(),
        source: format!("dut-{}", core),
        run_id: Some(run_id.to_string()),
        event,
    };
    for u in &rep.upsets {
        let _ = tx.send(mk(Event::Upset {
            core: core.to_string(),
            test: test.to_string(),
            scrub: rep.scrub,
            offset: u.offset as u64,
            expected: format!("{:016x}", u.expected),
            actual: format!("{:016x}", u.actual),
            flipped_bits: u.flipped_bits(),
            persistent: u.persistent,
        }));
    }
    agg.0 += 1;
    agg.1 += rep.bad_words as u64;
    agg.2 += rep.bad_bits;
    if agg.0 >= summary_every || rep.bad_words > 0 {
        let _ = tx.send(mk(Event::Scrub {
            core: core.to_string(),
            test: test.to_string(),
            bytes: region.bytes() as u64,
            passes: agg.0,
            bad_words: agg.1,
            bad_bits: agg.2,
            duration_us: dur,
        }));
        *agg = (0, 0, 0);
    }
}

fn spawn_scrubbers(tx: &Sender<Packet>, core: &str, run_id: &str, cpu: usize) {
    // Large DRAM-resident region: held for the whole run, scrubbed slowly so
    // upsets accumulate between reads (DRAM cells keep flips until rewritten).
    let mem_bytes = mem_region_bytes();
    let mem_interval = Duration::from_secs(env_u64("RADBENCH_MEM_SCRUB_S", 10).max(1));
    {
        let (tx, core, run_id) = (tx.clone(), core.to_string(), run_id.to_string());
        thread::spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: cpu });
            let mut region = MemRegion::new(mem_bytes, 6, 64);
            eprintln!(
                "[dut] mem scrub region: {} MiB every {:?}",
                region.bytes() >> 20,
                mem_interval
            );
            let mut agg = (0, 0, 0);
            loop {
                thread::sleep(mem_interval);
                if MEM_INJECT_PENDING.swap(false, Ordering::SeqCst) {
                    let word = (now_ms() as usize).wrapping_mul(2_654_435_761) % (region.bytes() / 8);
                    region.flip_bit(word, (now_ms() % 64) as u32);
                    eprintln!("[dut] synthetic memory fault injected at word {}", word);
                }
                scrub_and_report(&mut region, "mem", &tx, &core, &run_id, &mut agg, 1);
            }
        });
    }

    // Small cache-resident region: re-read often enough to stay in L1/L2 so
    // flips in the SRAM cache arrays are observed before the line is evicted
    // (an upset in a clean line that gets evicted is silently lost).
    let cache_kb = env_u64("RADBENCH_CACHE_KB", 16) as usize;
    if cache_kb > 0 {
        let interval = Duration::from_millis(env_u64("RADBENCH_CACHE_SCRUB_MS", 20).max(1));
        let summary_every = (10_000 / interval.as_millis().max(1) as u64).max(1);
        let (tx, core, run_id) = (tx.clone(), core.to_string(), run_id.to_string());
        thread::spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: cpu });
            let mut region = MemRegion::new(cache_kb << 10, 500, 64);
            eprintln!("[dut] cache scrub region: {} KiB every {:?}", cache_kb, interval);
            let mut agg = (0, 0, 0);
            loop {
                thread::sleep(interval);
                scrub_and_report(&mut region, "cache", &tx, &core, &run_id, &mut agg, summary_every);
            }
        });
    }
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

fn detect_arch() -> String {
    Command::new("uname")
        .arg("-m")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

fn detect_uname() -> String {
    Command::new("uname")
        .arg("-a")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}
