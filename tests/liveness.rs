use radbench::liveness::{Liveness, LivenessAlarm, LivenessConfig};

fn live() -> Liveness {
    Liveness::new(LivenessConfig::default())
}

fn hb(l: &mut Liveness, t: u64, iter: u64) -> Vec<LivenessAlarm> {
    let mut v = l.on_packet("dut-a", Some("r1"), t);
    v.extend(l.on_heartbeat("dut-a", t, 0, iter, t));
    v
}

#[test]
fn healthy_stream_raises_nothing() {
    let mut l = live();
    for s in 0..60 {
        assert!(hb(&mut l, s * 1000, s * 1_000_000).is_empty());
        assert!(l.tick(s * 1000 + 500).is_empty());
    }
}

#[test]
fn silence_alarms_once_then_recovers() {
    let mut l = live();
    hb(&mut l, 0, 0);
    assert!(l.tick(4_000).is_empty());
    assert!(matches!(l.tick(5_000)[..], [LivenessAlarm::Silent { silent_ms: 5_000, .. }]));
    assert!(l.tick(9_000).is_empty(), "only one alarm per outage");
    let v = hb(&mut l, 20_000, 1);
    assert!(matches!(v[..], [LivenessAlarm::Recovered { outage_ms: 20_000, .. }]));
}

#[test]
fn stalled_iteration_counter_alarms() {
    let mut l = live();
    for s in 0..10 {
        hb(&mut l, s * 1000, 42);
    }
    let v = l.tick(9_000);
    assert!(matches!(v[..], [LivenessAlarm::Stalled { iter: 42, .. }]), "{v:?}");
    assert!(l.tick(9_500).is_empty());
    // progress resumes -> re-armed
    hb(&mut l, 10_000, 43);
    assert!(l.tick(10_500).is_empty());
}

#[test]
fn slowdown_alarms() {
    let mut l = live();
    for s in 0..10 {
        hb(&mut l, s * 1000, s * 1000);
    }
    let v = hb(&mut l, 10_000, 9_000 + 200);
    assert!(matches!(v[..], [LivenessAlarm::Slow { .. }]), "{v:?}");
}

#[test]
fn epoch_rollover_is_not_a_slowdown() {
    let mut l = live();
    for s in 0..10 {
        hb(&mut l, s * 1000, s * 1000);
    }
    l.on_packet("dut-a", Some("r1"), 10_000);
    assert!(l.on_heartbeat("dut-a", 10_000, 1, 5, 10_000).is_empty());
}

#[test]
fn new_run_id_is_a_restart() {
    let mut l = live();
    hb(&mut l, 0, 0);
    let v = l.on_packet("dut-a", Some("r2"), 1000);
    assert!(matches!(&v[..], [LivenessAlarm::Restart { old_run, new_run, .. }] if old_run == "r1" && new_run == "r2"));
}

#[test]
fn recovery_does_not_immediately_report_stall() {
    let mut l = live();
    hb(&mut l, 0, 7);
    assert_eq!(l.tick(6_000).len(), 1); // silent
    hb(&mut l, 8_000, 7); // stale backlog heartbeat after outage
    assert!(l.tick(8_100).is_empty());
}
