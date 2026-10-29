//! On-disk formats.
//!
//! **Classifier format.** Every 512-byte sector is
//! `crc32c(4) | epoch(2) | batch(2) | address(4) | payload(500)`, the CRC
//! covering bytes 4..512. A batch (the unit of one fsync) starts on a fresh
//! sector; its payload stream is a header `magic(2) | sectors(2) |
//! records(2) | reserved(2)` followed by records `len(4) | crc(4) | payload`,
//! with the record CRC over `epoch | lsn | payload`. The LSN is implicit.
//!
//! **Baseline format.** A plain record stream `len(4) | lsn(4) | crc(4) |
//! payload`, CRC over `epoch | lsn | payload`; a record header never
//! straddles a sector (the gap is zero padding) and each batch is padded with
//! zeros to the next sector. No per-sector evidence.

use crate::crc::crc32c;

/// Sector size.
pub const SECTOR: usize = 512;
/// Classifier sector header size.
pub const SHDR: usize = 12;
/// Classifier sector payload size.
pub const SPAY: usize = SECTOR - SHDR;
/// Classifier batch header magic.
pub const MAGIC: u16 = 0xC1A5;
/// Classifier batch header size.
pub const BHDR: usize = 8;
/// Classifier record header size.
pub const CREC: usize = 8;
/// Baseline record header size.
pub const BREC: usize = 12;
/// Largest record payload accepted by either reader.
pub const MAX_PAYLOAD: usize = 1 << 20;

/// Which on-disk format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Per-sector evidence.
    Classifier,
    /// Per-record CRC only.
    Baseline,
}

/// Record CRC shared by both formats.
pub fn record_crc(epoch: u16, lsn: u64, payload: &[u8]) -> u32 {
    let mut buf = Vec::with_capacity(10 + payload.len());
    buf.extend_from_slice(&epoch.to_le_bytes());
    buf.extend_from_slice(&lsn.to_le_bytes());
    buf.extend_from_slice(payload);
    crc32c(&buf)
}

pub(crate) fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub(crate) fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Seal a classifier sector in place.
pub fn seal_sector(s: &mut [u8], epoch: u16, batch: u16, addr: u32) {
    s[4..6].copy_from_slice(&epoch.to_le_bytes());
    s[6..8].copy_from_slice(&batch.to_le_bytes());
    s[8..12].copy_from_slice(&addr.to_le_bytes());
    let c = crc32c(&s[4..SECTOR]);
    s[0..4].copy_from_slice(&c.to_le_bytes());
}

/// Encode one batch starting at sector `first_sector`. Returns the sector
/// images, in order.
pub fn encode_batch(
    fmt: Format,
    epoch: u16,
    batch: u16,
    first_lsn: u64,
    first_sector: u32,
    payloads: &[Vec<u8>],
) -> Vec<[u8; SECTOR]> {
    match fmt {
        Format::Classifier => {
            let body: usize = payloads.iter().map(|p| CREC + p.len()).sum();
            let nsect = (BHDR + body).div_ceil(SPAY);
            assert!(nsect <= u16::MAX as usize && payloads.len() <= u16::MAX as usize);
            let mut stream = Vec::with_capacity(nsect * SPAY);
            stream.extend_from_slice(&MAGIC.to_le_bytes());
            stream.extend_from_slice(&(nsect as u16).to_le_bytes());
            stream.extend_from_slice(&(payloads.len() as u16).to_le_bytes());
            stream.extend_from_slice(&[0, 0]);
            for (k, p) in payloads.iter().enumerate() {
                stream.extend_from_slice(&(p.len() as u32).to_le_bytes());
                let c = record_crc(epoch, first_lsn + k as u64, p);
                stream.extend_from_slice(&c.to_le_bytes());
                stream.extend_from_slice(p);
            }
            stream.resize(nsect * SPAY, 0);
            (0..nsect)
                .map(|k| {
                    let mut s = [0u8; SECTOR];
                    s[SHDR..].copy_from_slice(&stream[k * SPAY..(k + 1) * SPAY]);
                    seal_sector(&mut s, epoch, batch, first_sector + k as u32);
                    s
                })
                .collect()
        }
        Format::Baseline => {
            let mut stream = Vec::new();
            for (k, p) in payloads.iter().enumerate() {
                let lsn = first_lsn + k as u64;
                // A record header never straddles a sector: the reader could
                // not tell it from padding.
                if stream.len() % SECTOR > SECTOR - BREC {
                    stream.resize(stream.len().next_multiple_of(SECTOR), 0);
                }
                stream.extend_from_slice(&(p.len() as u32).to_le_bytes());
                stream.extend_from_slice(&(lsn as u32).to_le_bytes());
                stream.extend_from_slice(&record_crc(epoch, lsn, p).to_le_bytes());
                stream.extend_from_slice(p);
            }
            let nsect = stream.len().div_ceil(SECTOR);
            stream.resize(nsect * SECTOR, 0);
            (0..nsect)
                .map(|k| {
                    let mut s = [0u8; SECTOR];
                    s.copy_from_slice(&stream[k * SECTOR..(k + 1) * SECTOR]);
                    s
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifier_sectors_carry_their_address() {
        let p = vec![vec![7u8; 900], vec![9u8; 300]];
        let s = encode_batch(Format::Classifier, 3, 5, 10, 40, &p);
        assert_eq!(s.len(), (BHDR + 2 * CREC + 1200).div_ceil(SPAY));
        for (k, sec) in s.iter().enumerate() {
            assert_eq!(rd32(sec, 0), crc32c(&sec[4..]));
            assert_eq!(rd16(sec, 4), 3);
            assert_eq!(rd16(sec, 6), 5);
            assert_eq!(rd32(sec, 8), 40 + k as u32);
        }
    }

    #[test]
    fn baseline_is_padded_to_sectors() {
        let p = vec![vec![1u8; 600]];
        let s = encode_batch(Format::Baseline, 1, 0, 0, 0, &p);
        assert_eq!(s.len(), 2);
        assert_eq!(rd32(&s[0], 0), 600);
        assert!(s[1][612 - 512..].iter().all(|&b| b == 0));
    }
}
