use radbench::memtest::MemRegion;

#[test]
fn clean_region_has_no_upsets() {
    let mut r = MemRegion::new(64 << 10, 3, 16);
    for _ in 0..10 {
        let rep = r.scrub();
        assert_eq!(rep.bad_words, 0);
        assert_eq!(rep.words_checked, (64 << 10) / 8);
    }
}

#[test]
fn single_flip_detected_reported_and_corrected() {
    let mut r = MemRegion::new(4096, 100, 16);
    r.flip_bit(10, 5);
    let rep = r.scrub();
    assert_eq!(rep.bad_words, 1);
    assert_eq!(rep.bad_bits, 1);
    let u = &rep.upsets[0];
    assert_eq!(u.offset, 80);
    assert_eq!(u.expected ^ u.actual, 1 << 5);
    assert!(!u.persistent);
    // corrected: next scrub is clean
    assert_eq!(r.scrub().bad_words, 0);
}

#[test]
fn every_bit_of_a_word_is_detected() {
    let mut r = MemRegion::new(4096, 1000, 16);
    for bit in 0..64 {
        r.flip_bit(3, bit);
        let rep = r.scrub();
        assert_eq!(rep.bad_words, 1, "bit {bit}");
    }
}

#[test]
fn repeated_failure_of_same_word_is_persistent() {
    let mut r = MemRegion::new(4096, 1000, 16);
    r.flip_bit(7, 1);
    assert!(!r.scrub().upsets[0].persistent);
    r.flip_bit(7, 1);
    assert!(r.scrub().upsets[0].persistent);
    r.scrub();
    r.flip_bit(7, 1);
    assert!(!r.scrub().upsets[0].persistent);
}

#[test]
fn polarity_inversion_keeps_region_consistent() {
    let mut r = MemRegion::new(8192, 2, 16);
    let before = r.expected(0);
    for _ in 0..2 {
        assert_eq!(r.scrub().bad_words, 0);
    }
    // after 2 scrubs the pattern is inverted
    assert_eq!(r.expected(0), !before);
    assert_eq!(r.scrub().bad_words, 0);
    r.flip_bit(0, 0);
    assert_eq!(r.scrub().bad_words, 1);
}

#[test]
fn pattern_is_balanced_and_address_unique() {
    let r = MemRegion::new(1 << 20, 10, 16);
    let n = r.bytes() / 8;
    let ones: u64 = (0..n).map(|i| r.expected(i).count_ones() as u64).sum();
    let frac = ones as f64 / (n as f64 * 64.0);
    assert!((0.45..0.55).contains(&frac), "ones fraction {frac}");
    let mut seen = std::collections::HashSet::new();
    for i in 0..n {
        assert!(seen.insert(r.expected(i)), "duplicate pattern at {i}");
    }
}

#[test]
fn reports_are_capped_but_counts_are_not() {
    let mut r = MemRegion::new(4096, 1000, 4);
    for w in 0..20 {
        r.flip_bit(w, 0);
    }
    let rep = r.scrub();
    assert_eq!(rep.bad_words, 20);
    assert_eq!(rep.upsets.len(), 4);
}
