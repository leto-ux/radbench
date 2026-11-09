use radbench::kmsg::{classify, parse_kmsg};

#[test]
fn parses_kmsg_record() {
    let (p, m) = parse_kmsg("3,1234,5678901,-;mmc0: error -110 whilst initialising SD card\n").unwrap();
    assert_eq!(p, 3);
    assert_eq!(m, "mmc0: error -110 whilst initialising SD card");
    // facility bits are masked off
    assert_eq!(parse_kmsg("14,1,1,-;hello").unwrap().0, 6);
    assert!(parse_kmsg("garbage").is_none());
}

#[test]
fn fault_messages_are_classified() {
    let cases = [
        (0, "Kernel panic - not syncing: Fatal exception", "panic"),
        (1, "Unable to handle kernel paging request at virtual address dead", "oops"),
        (0, "Internal error: Oops: 96000004 [#1] PREEMPT SMP", "oops"),
        (6, "dut[123]: unhandled signal 11 code 0x1 at 0x0", "segfault"),
        (6, "dut[42]: segfault at 0 ip 0 sp 0 error 4", "segfault"),
        (3, "EDAC MC0: 1 CE memory read error", "edac"),
        (0, "watchdog: BUG: soft lockup - CPU#0 stuck for 22s!", "lockup"),
        (3, "rcu: INFO: rcu_sched detected stalls on CPUs/tasks", "lockup"),
        (3, "blk_update_request: I/O error, dev mmcblk0, sector 1234", "io"),
        (2, "EXT4-fs error (device mmcblk0p2): bad inode", "fs"),
        (3, "some driver failure nobody anticipated", "kernel_err"),
    ];
    for (prio, msg, cat) in cases {
        assert_eq!(classify(prio, msg), Some(cat), "{msg}");
    }
}

#[test]
fn routine_messages_are_ignored() {
    for msg in [
        "mmc0: new high speed SDHC card at address aaaa",
        "usbcore: registered new interface driver usbfs",
        "random: crng init done",
        "IPv6: ADDRCONF(NETDEV_CHANGE): usb0: link becomes ready",
    ] {
        assert_eq!(classify(6, msg), None, "{msg}");
    }
}
