//! wal-tail-classifier: a single-node WAL whose recovery tells a torn tail
//! (a batch whose fsync never returned) from corruption of acknowledged
//! records.
//!
//! Every 512-byte sector carries a CRC, the log epoch, the batch sequence
//! number and its own address ([`format`]). Recovery ([`recover`]) uses that
//! redundant evidence to decide whether damage can only come from an
//! interrupted write or must be corruption of data that was acknowledged.
//! The baseline format has one CRC per record and treats damage at the end
//! of the log as a torn tail, like most WALs do.
//!
//! [`trial`] runs the pre-registered crash experiment from `prereg.toml`.

pub mod control;
pub mod crc;
pub mod format;
pub mod prereg;
pub mod recover;
pub mod rng;
pub mod trial;
