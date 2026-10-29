//! CLI for the pre-registered experiment.

use std::process::ExitCode;
use std::time::Instant;

use wal_tail_classifier::control::mmap_control;
use wal_tail_classifier::prereg::Prereg;
use wal_tail_classifier::trial::{run_many, Fault, Tally};

fn usage() -> ExitCode {
    eprintln!(
        "usage:\n  wal-tail-classifier main [--threads N] [--shard I/N]\n  wal-tail-classifier negative [--threads N]\n  wal-tail-classifier dev [--threads N]\n  wal-tail-classifier mmap-control"
    );
    ExitCode::from(2)
}

fn pct(a: u64, b: u64) -> f64 {
    if b == 0 {
        f64::NAN
    } else {
        100.0 * a as f64 / b as f64
    }
}

fn line(name: &str, t: &Tally) -> String {
    format!(
        "{name}: torn={} torn_recovered={} ({:.4}%) torn_false_report={} corrupt={} detected={} ({:.4}%) missed_harmless={} silent_loss={} not_applicable={} none={} none_reports={} bytes={}",
        t.torn,
        t.torn_recovered,
        pct(t.torn_recovered, t.torn),
        t.torn_false_report,
        t.corrupt,
        t.corrupt_detected,
        pct(t.corrupt_detected, t.corrupt),
        t.corrupt_missed_harmless,
        t.silent_loss,
        t.not_applicable,
        t.none,
        t.none_reports,
        t.bytes
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().cloned() else {
        return usage();
    };
    let mut threads = 4usize;
    let mut shard = (0u64, 1u64);
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--threads" => threads = it.next().and_then(|v| v.parse().ok()).unwrap_or(4),
            "--shard" => {
                let Some((i, n)) = it.next().and_then(|v| v.split_once('/')) else {
                    return usage();
                };
                match (i.parse(), n.parse()) {
                    (Ok(i), Ok(n)) if i < n => shard = (i, n),
                    _ => return usage(),
                }
            }
            _ => return usage(),
        }
    }
    let p = Prereg::embedded();
    match cmd.as_str() {
        "main" => {
            let start0 = p.int("run.main_seeds_start");
            let total = p.int("run.main_seeds_count");
            let per = total.div_ceil(shard.1);
            let start = start0 + shard.0 * per;
            let count = per.min(start0 + total - start);
            let t0 = Instant::now();
            let (c, b, payload, bad) = run_many(&p, start, count, None, threads);
            let ovh = c.bytes as f64 / b.bytes as f64 - 1.0;
            println!(
                "seeds={}..{} wall_s={:.2}",
                start,
                start + count,
                t0.elapsed().as_secs_f64()
            );
            println!("{}", line("classifier", &c));
            println!("{}", line("baseline", &b));
            println!(
                "bytes_overhead_vs_baseline={:.4}% classifier_vs_payload={:.4}% baseline_vs_payload={:.4}% payload_bytes={payload}",
                100.0 * ovh,
                100.0 * (c.bytes as f64 / payload as f64 - 1.0),
                100.0 * (b.bytes as f64 / payload as f64 - 1.0)
            );
            println!("classifier_failing_seeds={bad:?}");
            let mut ok = true;
            let det = c.corrupt_detected as f64 / c.corrupt as f64;
            let rec = c.torn_recovered as f64 / c.torn as f64;
            if c.corrupt == 0
                || det < p.get("thresholds.confirmed_corruption_detected_min")
                || c.silent_loss > 0
            {
                eprintln!("FAIL: corruption detection {det}");
                ok = false;
            }
            if c.torn == 0 || rec < p.get("thresholds.torn_tail_recovered_min") {
                eprintln!("FAIL: torn tail recovery {rec}");
                ok = false;
            }
            if ovh > p.get("thresholds.bytes_overhead_max") {
                eprintln!("FAIL: bytes overhead {ovh}");
                ok = false;
            }
            if b.silent_loss == 0 {
                eprintln!("FAIL: positive control, the baseline lost nothing silently");
                ok = false;
            }
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        "negative" => {
            let start = p.int("run.negative_control_seeds_start");
            let count = p.int("run.negative_control_seeds_count");
            let (c, b, _, bad) = run_many(&p, start, count, Some(Fault::None), threads);
            println!("negative seeds={}..{}", start, start + count);
            println!("{}", line("classifier", &c));
            println!("{}", line("baseline", &b));
            println!("classifier_failing_seeds={bad:?}");
            if c.none_reports > p.int("thresholds.negative_control_reports_max") || c.none != count
            {
                eprintln!("FAIL: negative control reported {}", c.none_reports);
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        "dev" => {
            // Development seeds, outside every pre-registered range. Used to
            // debug before running the registered seeds; never reported as a
            // result.
            let (c, b, _, bad) = run_many(&p, 9_000_000, 2_000, None, threads);
            println!("dev seeds=9000000..9002000 (not pre-registered)");
            println!("{}", line("classifier", &c));
            println!("{}", line("baseline", &b));
            println!("classifier_failing_seeds={bad:?}");
            ExitCode::SUCCESS
        }
        "mmap-control" => {
            let start = p.int("run.mmap_wal_log_control_seeds_start");
            let count = p.int("run.mmap_wal_log_control_seeds_count");
            let r = mmap_control(&p, start, count);
            println!(
                "mmap-wal-log@739ec7e trials={} silent_truncation_of_acknowledged_record={} refused={} other={}",
                r.trials, r.silent, r.refused, r.other
            );
            if r.silent == 0 {
                eprintln!("FAIL: positive control expected silent truncations");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}
