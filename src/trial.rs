//! The pre-registered crash experiment.

use crate::format::{encode_batch, Format, SECTOR};
use crate::prereg::Prereg;
use crate::recover::{recover_baseline, recover_classifier, Outcome, Recovery};
use crate::rng::Rng;

/// Current epoch of the log under test; recycled sectors carry `EPOCH - 1`.
pub const EPOCH: u16 = 2;

/// Injected fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// No fault (negative control).
    None,
    /// Interrupted write of an unacknowledged batch.
    Torn,
    /// Bits flipped in an acknowledged sector.
    BitFlip,
    /// An acknowledged sector overwritten with another sector's image.
    Misdirected,
}

/// How one format fared in one trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Score {
    /// No fault or torn tail, and every acknowledged record came back.
    Recovered,
    /// Corruption reported as corruption.
    Detected,
    /// Torn tail or no fault reported as corruption (not automatic).
    FalseReport,
    /// Corruption not reported, but no acknowledged record lost.
    MissedHarmless,
    /// Acknowledged records lost or altered without a report.
    SilentLoss,
    /// Misdirected write with fewer than two acknowledged sectors.
    NotApplicable,
}

/// One trial, both formats.
#[derive(Debug, Clone, Copy)]
pub struct TrialResult {
    /// Seed.
    pub seed: u64,
    /// Fault.
    pub fault: Fault,
    /// Recycled log region.
    pub recycled: bool,
    /// Classifier score.
    pub classifier: Score,
    /// Baseline score.
    pub baseline: Score,
    /// Bytes written for acknowledged batches, classifier format.
    pub bytes_classifier: u64,
    /// Same, baseline format.
    pub bytes_baseline: u64,
    /// Payload bytes acknowledged.
    pub payload_bytes: u64,
}

struct Workload {
    acked: Vec<Vec<Vec<u8>>>,
    inflight: Vec<Vec<u8>>,
}

fn batch(rng: &mut Rng, p: &Prereg) -> Vec<Vec<u8>> {
    let n = rng.between(
        p.int("workload.records_per_batch_min"),
        p.int("workload.records_per_batch_max"),
    );
    (0..n)
        .map(|_| {
            let len = rng.between(p.int("workload.payload_min"), p.int("workload.payload_max"));
            let mut v = vec![0u8; len as usize];
            let mut x = rng.next_u64();
            for b in v.iter_mut() {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                *b = (x >> 56) as u8;
            }
            v
        })
        .collect()
}

/// Lay out `batches` from sector 0. Returns sector images and, per batch,
/// its sector range.
fn layout(
    fmt: Format,
    epoch: u16,
    batches: &[Vec<Vec<u8>>],
) -> (Vec<[u8; SECTOR]>, Vec<(usize, usize)>) {
    let mut out = Vec::new();
    let mut ranges = Vec::new();
    let mut lsn = 0u64;
    for (b, p) in batches.iter().enumerate() {
        let first = out.len();
        out.extend(encode_batch(fmt, epoch, b as u16, lsn, first as u32, p));
        ranges.push((first, out.len()));
        lsn += p.len() as u64;
    }
    (out, ranges)
}

fn recover(fmt: Format, dev: &[u8]) -> Recovery {
    match fmt {
        Format::Classifier => recover_classifier(dev, EPOCH),
        Format::Baseline => recover_baseline(dev, EPOCH),
    }
}

/// Run one format of one trial. `frng` drives format-specific choices.
fn run_format(
    fmt: Format,
    p: &Prereg,
    w: &Workload,
    fault: Fault,
    recycled: bool,
    seed: u64,
    frng: &mut Rng,
) -> (Score, u64) {
    let mut all = w.acked.clone();
    if fault == Fault::Torn {
        all.push(w.inflight.clone());
    }
    let (sectors, ranges) = layout(fmt, EPOCH, &all);
    let acked_sectors = ranges[w.acked.len() - 1].1;
    let bytes = (acked_sectors * SECTOR) as u64;
    let size = sectors.len() + 16;
    // Device initial content.
    let mut dev = vec![0u8; size * SECTOR];
    if recycled {
        let mut orng = Rng::new(seed ^ 0x5EED_0F01_D106);
        let mut old = Vec::new();
        let mut old_sectors = 0usize;
        while old_sectors < size + size / 4 + 2 {
            let b = batch(&mut orng, p);
            old_sectors += b.iter().map(|r| r.len() + 16).sum::<usize>() / (SECTOR - 16) + 1;
            old.push(b);
        }
        let (os, _) = layout(fmt, EPOCH - 1, &old);
        for (i, s) in os.iter().take(size).enumerate() {
            dev[i * SECTOR..(i + 1) * SECTOR].copy_from_slice(s);
        }
    }
    for (i, s) in sectors.iter().take(acked_sectors).enumerate() {
        dev[i * SECTOR..(i + 1) * SECTOR].copy_from_slice(s);
    }
    let mut applicable = true;
    match fault {
        Fault::None => {}
        Fault::Torn => {
            let unit = if frng.chance(0.5) { 1 } else { 8 };
            let pp = p.get("faults.torn_unit_persist_p");
            let (a, z) = ranges[w.acked.len()];
            let mut u = a - a % unit;
            while u < z {
                let persist = frng.chance(pp);
                for i in u.max(a)..(u + unit).min(z) {
                    if persist {
                        dev[i * SECTOR..(i + 1) * SECTOR].copy_from_slice(&sectors[i]);
                    }
                }
                u += unit;
            }
        }
        Fault::BitFlip | Fault::Misdirected => {
            let last = ranges[w.acked.len() - 1];
            let pick = |r: &mut Rng| -> usize {
                if r.chance(p.get("faults.corrupt_last_batch_p")) {
                    r.between(last.0 as u64, last.1 as u64 - 1) as usize
                } else {
                    r.between(0, acked_sectors as u64 - 1) as usize
                }
            };
            let t = pick(frng);
            if fault == Fault::BitFlip {
                let nbits = frng.between(
                    p.int("faults.bit_flip_bits_min"),
                    p.int("faults.bit_flip_bits_max"),
                );
                let mut used: Vec<u64> = Vec::new();
                while (used.len() as u64) < nbits {
                    let bit = frng.between(0, (SECTOR * 8) as u64 - 1);
                    if !used.contains(&bit) {
                        used.push(bit);
                        dev[t * SECTOR + (bit / 8) as usize] ^= 1 << (bit % 8);
                    }
                }
            } else if acked_sectors < 2 {
                applicable = false;
            } else {
                let mut s = frng.between(0, acked_sectors as u64 - 2) as usize;
                if s >= t {
                    s += 1;
                }
                let img = sectors[s];
                dev[t * SECTOR..(t + 1) * SECTOR].copy_from_slice(&img);
            }
        }
    }
    if !applicable {
        return (Score::NotApplicable, bytes);
    }
    let r = recover(fmt, &dev);
    let acked: Vec<&Vec<u8>> = w.acked.iter().flatten().collect();
    let prefix_ok =
        r.records.len() >= acked.len() && acked.iter().zip(r.records.iter()).all(|(a, b)| *a == b);
    let extra_ok = r.records[acked.len().min(r.records.len())..]
        .iter()
        .zip(w.inflight.iter())
        .all(|(a, b)| a == b)
        && r.records.len()
            <= acked.len()
                + if fault == Fault::Torn {
                    w.inflight.len()
                } else {
                    0
                };
    let intact = prefix_ok && extra_ok;
    let corrupt = matches!(r.outcome, Outcome::Corrupt { .. });
    let score = match fault {
        Fault::None | Fault::Torn => {
            // Without a fault the classifier must say Clean: a torn-tail
            // finding is a report. The baseline cannot tell the end of the
            // log from stale recycled data, so its "torn" is a normal stop.
            let spurious_torn =
                fault == Fault::None && fmt == Format::Classifier && r.outcome == Outcome::Torn;
            if corrupt || spurious_torn {
                Score::FalseReport
            } else if intact {
                Score::Recovered
            } else {
                Score::SilentLoss
            }
        }
        Fault::BitFlip | Fault::Misdirected => {
            if corrupt {
                Score::Detected
            } else if intact {
                Score::MissedHarmless
            } else {
                Score::SilentLoss
            }
        }
    };
    (score, bytes)
}

/// Run trial `seed`. `force` overrides the fault draw (negative control).
pub fn run(p: &Prereg, seed: u64, force: Option<Fault>) -> TrialResult {
    let mut rng = Rng::new(seed);
    let u = (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    let drawn = if u < p.get("faults.torn_tail") {
        Fault::Torn
    } else if u < p.get("faults.torn_tail") + p.get("faults.bit_flip") {
        Fault::BitFlip
    } else {
        Fault::Misdirected
    };
    let fault = force.unwrap_or(drawn);
    let recycled = rng.chance(p.get("device.recycled_fraction"));
    let nb = rng.between(
        p.int("workload.acked_batches_min"),
        p.int("workload.acked_batches_max"),
    );
    let acked: Vec<Vec<Vec<u8>>> = (0..nb).map(|_| batch(&mut rng, p)).collect();
    let inflight = batch(&mut rng, p);
    let w = Workload { acked, inflight };
    let payload_bytes = w.acked.iter().flatten().map(|r| r.len() as u64).sum();
    let mut rc = Rng::new(seed.wrapping_mul(31).wrapping_add(1));
    let mut rb = Rng::new(seed.wrapping_mul(31).wrapping_add(2));
    let (classifier, bytes_classifier) =
        run_format(Format::Classifier, p, &w, fault, recycled, seed, &mut rc);
    let (baseline, bytes_baseline) =
        run_format(Format::Baseline, p, &w, fault, recycled, seed, &mut rb);
    TrialResult {
        seed,
        fault,
        recycled,
        classifier,
        baseline,
        bytes_classifier,
        bytes_baseline,
        payload_bytes,
    }
}

/// Totals for one format.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tally {
    /// Torn-tail trials.
    pub torn: u64,
    /// Torn tails recovered.
    pub torn_recovered: u64,
    /// Torn tails reported as corruption.
    pub torn_false_report: u64,
    /// Corruption trials (applicable).
    pub corrupt: u64,
    /// Corruptions detected.
    pub corrupt_detected: u64,
    /// Corruptions missed without loss.
    pub corrupt_missed_harmless: u64,
    /// Silent losses (any fault).
    pub silent_loss: u64,
    /// Fault-free trials.
    pub none: u64,
    /// Fault-free trials with any report or loss.
    pub none_reports: u64,
    /// Misdirected trials not applicable.
    pub not_applicable: u64,
    /// Bytes written for acknowledged batches.
    pub bytes: u64,
}

impl Tally {
    /// Fold one score in.
    pub fn add(&mut self, fault: Fault, s: Score, bytes: u64) {
        self.bytes += bytes;
        if s == Score::SilentLoss {
            self.silent_loss += 1;
        }
        match fault {
            Fault::None => {
                self.none += 1;
                if s != Score::Recovered {
                    self.none_reports += 1;
                }
            }
            Fault::Torn => {
                self.torn += 1;
                match s {
                    Score::Recovered => self.torn_recovered += 1,
                    Score::FalseReport => self.torn_false_report += 1,
                    _ => {}
                }
            }
            Fault::BitFlip | Fault::Misdirected => {
                if s == Score::NotApplicable {
                    self.not_applicable += 1;
                    return;
                }
                self.corrupt += 1;
                match s {
                    Score::Detected => self.corrupt_detected += 1,
                    Score::MissedHarmless => self.corrupt_missed_harmless += 1,
                    _ => {}
                }
            }
        }
    }
}

/// Run seeds `start..start + count` on `threads` threads.
pub fn run_many(
    p: &Prereg,
    start: u64,
    count: u64,
    force: Option<Fault>,
    threads: usize,
) -> (Tally, Tally, u64, Vec<u64>) {
    let threads = threads.max(1) as u64;
    let chunks: Vec<(Tally, Tally, u64, Vec<u64>)> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..threads)
            .map(|t| {
                sc.spawn(move || {
                    let (mut c, mut b, mut payload, mut bad) =
                        (Tally::default(), Tally::default(), 0u64, Vec::new());
                    let mut seed = start + t;
                    while seed < start + count {
                        let r = run(p, seed, force);
                        c.add(r.fault, r.classifier, r.bytes_classifier);
                        b.add(r.fault, r.baseline, r.bytes_baseline);
                        payload += r.payload_bytes;
                        let fails = matches!(
                            r.classifier,
                            Score::SilentLoss | Score::FalseReport | Score::MissedHarmless
                        );
                        if fails && bad.len() < 10 {
                            bad.push(seed);
                        }
                        seed += threads;
                    }
                    (c, b, payload, bad)
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("worker")).collect()
    });
    let mut c = Tally::default();
    let mut b = Tally::default();
    let mut payload = 0;
    let mut bad = Vec::new();
    for (x, y, pl, bd) in chunks {
        for (acc, t) in [(&mut c, x), (&mut b, y)] {
            acc.torn += t.torn;
            acc.torn_recovered += t.torn_recovered;
            acc.torn_false_report += t.torn_false_report;
            acc.corrupt += t.corrupt;
            acc.corrupt_detected += t.corrupt_detected;
            acc.corrupt_missed_harmless += t.corrupt_missed_harmless;
            acc.silent_loss += t.silent_loss;
            acc.none += t.none;
            acc.none_reports += t.none_reports;
            acc.not_applicable += t.not_applicable;
            acc.bytes += t.bytes;
        }
        payload += pl;
        bad.extend(bd);
    }
    bad.sort_unstable();
    bad.truncate(10);
    (c, b, payload, bad)
}
