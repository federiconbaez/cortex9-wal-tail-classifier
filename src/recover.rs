//! Recovery for both formats.

use crate::crc::crc32c;
use crate::format::{rd16, rd32, record_crc, BHDR, BREC, CREC, MAGIC, MAX_PAYLOAD, SECTOR, SHDR};

/// What recovery concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The log ends cleanly.
    Clean,
    /// A torn tail was found and dropped.
    Torn,
    /// Damage that cannot come from an interrupted write. Recovery refuses
    /// to truncate; `sector` is where the evidence is.
    Corrupt {
        /// Sector index.
        sector: usize,
    },
}

/// Recovery result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    /// Conclusion.
    pub outcome: Outcome,
    /// Records the log continues with (for `Corrupt`, the readable prefix).
    pub records: Vec<Vec<u8>>,
}

/// Classification of one classifier-format sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// All zero: never written.
    Zero,
    /// CRC fails and not all zero.
    BadCrc,
    /// Valid sector of the current epoch at its own address.
    Cur {
        /// Batch sequence.
        batch: u16,
    },
    /// Valid sector of another epoch at its own address (recycled).
    Old,
    /// Valid sector whose address field is not where it sits.
    Misdirected,
}

/// Classify sector `i`.
pub fn class(dev: &[u8], i: usize, epoch: u16) -> Class {
    let s = &dev[i * SECTOR..(i + 1) * SECTOR];
    if rd32(s, 0) != crc32c(&s[4..]) {
        return if s.iter().all(|&b| b == 0) {
            Class::Zero
        } else {
            Class::BadCrc
        };
    }
    if rd32(s, 8) as usize != i {
        return Class::Misdirected;
    }
    if rd16(s, 4) == epoch {
        Class::Cur { batch: rd16(s, 6) }
    } else {
        Class::Old
    }
}

/// Recover a classifier-format log of epoch `epoch`.
pub fn recover_classifier(dev: &[u8], epoch: u16) -> Recovery {
    let n = dev.len() / SECTOR;
    let classes: Vec<Class> = (0..n).map(|i| class(dev, i, epoch)).collect();
    let mut records: Vec<Vec<u8>> = Vec::new();
    let corrupt = |sector: usize, records: Vec<Vec<u8>>| Recovery {
        outcome: Outcome::Corrupt { sector },
        records,
    };
    // Under the model an interrupted write only leaves whole sectors that are
    // new, old or zero. A bad CRC or a sector at the wrong address can only be
    // corruption, wherever it is.
    if let Some(i) = classes
        .iter()
        .position(|c| matches!(c, Class::BadCrc | Class::Misdirected))
    {
        return corrupt(i, records);
    }
    let mut i = 0usize;
    let mut b: u16 = 0;
    loop {
        if i >= n {
            return Recovery {
                outcome: Outcome::Clean,
                records,
            };
        }
        let incomplete_from = match classes[i] {
            Class::Cur { batch } if batch == b => {
                let s = &dev[i * SECTOR + SHDR..(i + 1) * SECTOR];
                let nsect = rd16(s, 2) as usize;
                let nrec = rd16(s, 4) as usize;
                if rd16(s, 0) != MAGIC || nsect == 0 {
                    // Valid sector, right epoch, batch and address, but not a
                    // batch header: nothing a torn write produces.
                    return corrupt(i, records);
                }
                let complete =
                    i + nsect <= n && (i..i + nsect).all(|j| classes[j] == Class::Cur { batch: b });
                if complete {
                    let mut stream = Vec::with_capacity(nsect * (SECTOR - SHDR));
                    for j in i..i + nsect {
                        stream.extend_from_slice(&dev[j * SECTOR + SHDR..(j + 1) * SECTOR]);
                    }
                    let mut off = BHDR;
                    for _ in 0..nrec {
                        if off + CREC > stream.len() {
                            return corrupt(i, records);
                        }
                        let len = rd32(&stream, off) as usize;
                        let crc = rd32(&stream, off + 4);
                        let lsn = records.len() as u64;
                        if len > MAX_PAYLOAD
                            || off + CREC + len > stream.len()
                            || record_crc(epoch, lsn, &stream[off + CREC..off + CREC + len]) != crc
                        {
                            return corrupt(i, records);
                        }
                        records.push(stream[off + CREC..off + CREC + len].to_vec());
                        off += CREC + len;
                    }
                    i += nsect;
                    b = b.wrapping_add(1);
                    continue;
                }
                i
            }
            Class::Cur { .. } => return corrupt(i, records),
            _ => i,
        };
        // Batch `b` is missing or partial. That is a torn tail only if no
        // later batch exists: batch b+1 is written after b's fsync returned.
        for (j, c) in classes.iter().enumerate().skip(incomplete_from) {
            if let Class::Cur { batch } = *c {
                if batch != b {
                    return corrupt(j, records);
                }
            }
        }
        let any_partial = classes[incomplete_from..]
            .iter()
            .any(|c| matches!(c, Class::Cur { .. }));
        return Recovery {
            outcome: if any_partial {
                Outcome::Torn
            } else {
                Outcome::Clean
            },
            records,
        };
    }
}

/// Recover a baseline-format log. Damage followed by a valid record is
/// reported as corruption (as `mmap-wal-log` does); damage with nothing
/// valid after it is treated as a torn tail and truncated.
pub fn recover_baseline(dev: &[u8], epoch: u16) -> Recovery {
    let mut records: Vec<Vec<u8>> = Vec::new();
    let mut pos = 0usize;
    let len_at = |p: usize| -> Option<u32> { (p + 4 <= dev.len()).then(|| rd32(dev, p)) };
    loop {
        if pos % SECTOR > SECTOR - BREC {
            pos = pos.next_multiple_of(SECTOR);
        }
        let Some(len) = len_at(pos) else {
            return Recovery {
                outcome: Outcome::Clean,
                records,
            };
        };
        if len == 0 {
            if pos % SECTOR == 0 {
                return Recovery {
                    outcome: Outcome::Clean,
                    records,
                };
            }
            pos = pos.next_multiple_of(SECTOR);
            continue;
        }
        let lsn = records.len() as u64;
        if let Some(p) = valid_baseline_record(dev, pos, epoch, lsn) {
            records.push(p.to_vec());
            pos += BREC + p.len();
            continue;
        }
        // Damage at `pos`. Anything valid later on a sector boundary?
        let mut s = pos.next_multiple_of(SECTOR).max(SECTOR);
        if s == pos {
            s += SECTOR;
        }
        while s + BREC <= dev.len() {
            let l = rd32(dev, s + 4) as u64;
            if l >= lsn && valid_baseline_record(dev, s, epoch, l).is_some() {
                return Recovery {
                    outcome: Outcome::Corrupt {
                        sector: pos / SECTOR,
                    },
                    records,
                };
            }
            s += SECTOR;
        }
        return Recovery {
            outcome: Outcome::Torn,
            records,
        };
    }
}

fn valid_baseline_record(dev: &[u8], pos: usize, epoch: u16, lsn: u64) -> Option<&[u8]> {
    if pos + BREC > dev.len() {
        return None;
    }
    let len = rd32(dev, pos) as usize;
    if len == 0 || len > MAX_PAYLOAD || pos + BREC + len > dev.len() {
        return None;
    }
    if rd32(dev, pos + 4) != lsn as u32 {
        return None;
    }
    let p = &dev[pos + BREC..pos + BREC + len];
    (record_crc(epoch, lsn, p) == rd32(dev, pos + 8)).then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{encode_batch, Format};

    fn build(fmt: Format, batches: &[Vec<Vec<u8>>], extra: usize) -> Vec<u8> {
        let mut dev = Vec::new();
        let mut lsn = 0;
        for (b, p) in batches.iter().enumerate() {
            let first = (dev.len() / SECTOR) as u32;
            for s in encode_batch(fmt, 2, b as u16, lsn, first, p) {
                dev.extend_from_slice(&s);
            }
            lsn += p.len() as u64;
        }
        dev.resize(dev.len() + extra * SECTOR, 0);
        dev
    }

    fn sample() -> Vec<Vec<Vec<u8>>> {
        vec![
            vec![vec![1u8; 700], vec![2u8; 300]],
            vec![vec![3u8; 1000]],
            vec![vec![4u8; 400], vec![5u8; 260], vec![6u8; 2048]],
        ]
    }

    #[test]
    fn clean_logs_recover_fully_in_both_formats() {
        for fmt in [Format::Classifier, Format::Baseline] {
            let dev = build(fmt, &sample(), 4);
            let r = match fmt {
                Format::Classifier => recover_classifier(&dev, 2),
                Format::Baseline => recover_baseline(&dev, 2),
            };
            assert_eq!(r.outcome, Outcome::Clean, "{fmt:?}");
            assert_eq!(r.records.len(), 6);
            assert_eq!(r.records[5], vec![6u8; 2048]);
        }
    }

    #[test]
    fn bit_flip_in_last_batch_is_corruption_not_tail() {
        let dev0 = build(Format::Classifier, &sample(), 4);
        let n = dev0.len() / SECTOR - 4;
        let mut dev = dev0.clone();
        dev[(n - 1) * SECTOR + 100] ^= 0x10;
        assert!(matches!(
            recover_classifier(&dev, 2).outcome,
            Outcome::Corrupt { .. }
        ));
        // The baseline truncates the same damage silently.
        let b0 = build(Format::Baseline, &sample(), 4);
        let nb = b0.len() / SECTOR - 4;
        let mut b = b0.clone();
        b[(nb - 1) * SECTOR + 100] ^= 0x10;
        let r = recover_baseline(&b, 2);
        assert_eq!(r.outcome, Outcome::Torn);
        assert_eq!(r.records.len(), 5, "acknowledged record 6 dropped silently");
    }

    #[test]
    fn torn_last_batch_is_dropped() {
        let mut batches = sample();
        batches.push(vec![vec![9u8; 1500]]);
        let mut dev = build(Format::Classifier, &batches, 2);
        let n = dev.len() / SECTOR - 2;
        // Last batch spans several sectors; lose its second one.
        let first_of_last = n - (BHDR + CREC + 1500).div_ceil(SECTOR - SHDR);
        let k = first_of_last + 1;
        dev[k * SECTOR..(k + 1) * SECTOR].fill(0);
        let r = recover_classifier(&dev, 2);
        assert_eq!(r.outcome, Outcome::Torn);
        assert_eq!(r.records.len(), 6);
    }

    #[test]
    fn misdirected_sector_is_corruption() {
        let mut dev = build(Format::Classifier, &sample(), 2);
        let img = dev[0..SECTOR].to_vec();
        dev[2 * SECTOR..3 * SECTOR].copy_from_slice(&img);
        assert_eq!(
            recover_classifier(&dev, 2).outcome,
            Outcome::Corrupt { sector: 2 }
        );
    }

    #[test]
    fn hole_in_an_earlier_batch_with_a_later_batch_present_is_corruption() {
        let mut dev = build(Format::Classifier, &sample(), 2);
        // Zero a sector of batch 0 while batches 1 and 2 exist.
        dev[SECTOR..2 * SECTOR].fill(0);
        assert!(matches!(
            recover_classifier(&dev, 2).outcome,
            Outcome::Corrupt { .. }
        ));
    }
}
