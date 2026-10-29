//! Positive control on the real `mmap-wal-log` (pinned at 739ec7e): append
//! and flush records (acknowledged), flip one bit in the last record's
//! payload, reopen.

use mmap_wal_log::{Log, Options, HEADER};

use crate::prereg::Prereg;
use crate::rng::Rng;

/// Control outcome counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ControlResult {
    /// Trials run.
    pub trials: u64,
    /// Reopened fine with fewer records: acknowledged record lost silently.
    pub silent: u64,
    /// Refused to open.
    pub refused: u64,
    /// Anything else.
    pub other: u64,
}

/// Run seeds `start..start + count`.
pub fn mmap_control(p: &Prereg, start: u64, count: u64) -> ControlResult {
    let dir = std::env::temp_dir().join(format!(
        "wal-tail-classifier-mmap-{}-{start}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut res = ControlResult {
        trials: count,
        ..ControlResult::default()
    };
    for seed in start..start + count {
        let mut r = Rng::new(seed);
        let path = dir.join(format!("{seed}.wal"));
        let n = r.between(1, 64);
        let mut last_off = 0u64;
        let mut last_len = 0usize;
        {
            let opts = Options {
                initial_capacity: 1 << 18,
                ..Options::default()
            };
            let (mut log, _) = Log::open(&path, opts).expect("open");
            for _ in 0..n {
                let len = r.between(p.int("workload.payload_min"), p.int("workload.payload_max"))
                    as usize;
                let payload: Vec<u8> = (0..len).map(|_| r.next_u64() as u8).collect();
                log.append(&payload).expect("append");
                last_len = len;
            }
            log.flush().expect("flush");
            if let Some(rec) = log.iter().last() {
                last_off = rec.offset;
            }
        }
        let mut bytes = std::fs::read(&path).expect("read");
        let at = last_off as usize + HEADER + r.between(0, last_len as u64 - 1) as usize;
        bytes[at] ^= 1 << r.between(0, 7);
        std::fs::write(&path, &bytes).expect("write");
        match Log::open(&path, Options::default()) {
            Ok((_, rep)) if rep.records < n => res.silent += 1,
            Ok(_) => res.other += 1,
            Err(_) => res.refused += 1,
        }
        std::fs::remove_file(&path).ok();
    }
    std::fs::remove_dir_all(&dir).ok();
    res
}
