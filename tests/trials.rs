//! Fast versions of the experiment on development seeds (outside every
//! pre-registered range). The registered runs are in CI and the README.

use wal_tail_classifier::prereg::Prereg;
use wal_tail_classifier::trial::{run, run_many, Fault, Score};

const DEV: u64 = 9_100_000;

#[test]
fn classifier_meets_thresholds_on_dev_seeds() {
    let p = Prereg::embedded();
    let (c, b, _, bad) = run_many(&p, DEV, 300, None, 2);
    assert!(bad.is_empty(), "{bad:?}");
    assert!(c.torn > 0 && c.corrupt > 0);
    assert_eq!(c.corrupt_detected, c.corrupt);
    assert_eq!(c.torn_recovered, c.torn);
    assert_eq!(c.silent_loss, 0);
    assert!(b.silent_loss > 0, "baseline must lose acknowledged records");
    let ovh = c.bytes as f64 / b.bytes as f64 - 1.0;
    assert!(ovh <= 0.05, "{ovh}");
}

#[test]
fn fault_free_trials_report_nothing() {
    let p = Prereg::embedded();
    for seed in DEV..DEV + 100 {
        let r = run(&p, seed, Some(Fault::None));
        assert_eq!(r.classifier, Score::Recovered, "seed {seed}");
        assert_eq!(r.baseline, Score::Recovered, "seed {seed}");
    }
}

#[test]
fn every_fault_kind_occurs_and_trials_are_deterministic() {
    let p = Prereg::embedded();
    let mut seen = [false; 3];
    for seed in DEV..DEV + 200 {
        let a = run(&p, seed, None);
        let b = run(&p, seed, None);
        assert_eq!(a.classifier, b.classifier);
        assert_eq!(a.baseline, b.baseline);
        assert_eq!(a.bytes_baseline, b.bytes_baseline);
        match a.fault {
            Fault::Torn => seen[0] = true,
            Fault::BitFlip => seen[1] = true,
            Fault::Misdirected => seen[2] = true,
            Fault::None => unreachable!(),
        }
    }
    assert_eq!(seen, [true; 3]);
}

#[test]
fn mmap_wal_log_truncates_a_corrupted_last_record_silently() {
    let p = Prereg::embedded();
    let r = wal_tail_classifier::control::mmap_control(&p, DEV, 20);
    assert_eq!(r.trials, 20);
    assert!(r.silent > 0, "{r:?}");
}
