// src/bin/monitor.rs
use radbench::protocol::{Event, Packet, Status};
use radbench::reference;
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::Duration;

fn main() {
    let listen = std::env::var("MONITOR_LISTEN").unwrap_or_else(|_| "0.0.0.0:9000".into());
    let serial = std::env::var("MONITOR_SERIAL").ok();

    println!("monitor self-test: recomputing reference hashes...");
    let reference = reference::checkpoints();
    for cp in reference {
        let recomputed = reference::hash_fib(cp.n);
        assert_eq!(
            recomputed, cp.expected_hash,
            "monitor self-test failed at n={}",
            cp.n
        );
    }
    println!("monitor self-test passed ({} checkpoints)", reference.len());

    let (tx, rx) = channel::<Packet>();
    let tx_tcp = tx.clone();

    // RJ45 listener
    thread::spawn(move || {
        let listener = TcpListener::bind(&listen).unwrap();
        println!("listening on {}", listen);
        for stream in listener.incoming() {
            if let Ok(s) = stream {
                let tx = tx_tcp.clone();
                thread::spawn(move || handle_stream(s, tx));
            }
        }
    });

    // UART listener
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
                        let _ = tx_serial.send(p);
                    }
                }
            }
        });
    }

    // Dedup key includes run_id so restarts with seq=1 don't collide
    let mut seen = HashSet::new();
    let mut last_heartbeat: HashMap<String, u64> = HashMap::new();
    let mut alarm = OpenOptions::new()
        .create(true)
        .append(true)
        .open("alarms.log")
        .unwrap();

    for pkt in rx {
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
            Event::Checkpoint {
                core,
                test,
                n,
                hash,
                status,
                ..
            } => {
                let expected = reference
                    .iter()
                    .find(|c| c.n == *n)
                    .map(|c| c.expected_hash);
                let ok = expected.map(|e| e == hash.as_str()).unwrap_or(false);
                if !ok || *status == Status::Mismatch {
                    let msg = format!(
                        "ALARM ts={}{} core={} test={} n={} expected={:?} got={}",
                        pkt.ts, run_tag, core, test, n, expected, hash
                    );
                    eprintln!("{}", msg);
                    writeln!(alarm, "{}", msg).unwrap();
                    alarm.flush().unwrap();
                }
            }
            Event::Heartbeat { core, iter, .. } => {
                last_heartbeat.insert(core.clone(), pkt.ts);
                eprintln!("heartbeat{} {} iter={}", run_tag, core, iter);
            }
            Event::Error { .. } => {
                let msg = format!("ALARM ts={}{} DUT-ERROR {:?}", pkt.ts, run_tag, pkt);
                eprintln!("{}", msg);
                writeln!(alarm, "{}", msg).unwrap();
                alarm.flush().unwrap();
            }
            Event::Shutdown {
                core,
                reason,
                final_iter,
            } => {
                let msg = format!(
                    "SHUTDOWN ts={}{} core={} reason={} final_iter={}",
                    pkt.ts, run_tag, core, reason, final_iter
                );
                eprintln!("{}", msg);
                writeln!(alarm, "{}", msg).unwrap();
                alarm.flush().unwrap();
            }
            _ => {}
        }
    }
}

fn handle_stream(stream: TcpStream, tx: Sender<Packet>) {
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        if let Ok(l) = line {
            if let Ok(p) = serde_json::from_str::<Packet>(&l) {
                let _ = tx.send(p);
            }
        }
    }
}
