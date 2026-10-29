//! Reads `prereg.toml` (flat `key = number` pairs under `[section]`). No
//! TOML crate: the file only holds numbers.

use std::collections::BTreeMap;

/// Parsed pre-registration.
#[derive(Debug, Clone, PartialEq)]
pub struct Prereg {
    /// `section.key` -> value.
    pub values: BTreeMap<String, f64>,
}

/// The file as committed, compiled in so a binary cannot silently use other
/// parameters.
pub const EMBEDDED: &str = include_str!("../prereg.toml");

impl Prereg {
    /// Parse text.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut values = BTreeMap::new();
        let mut section = String::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = s.trim().to_string();
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| format!("line {}: expected key = value", n + 1))?;
            let v: f64 = v
                .trim()
                .parse()
                .map_err(|_| format!("line {}: not a number", n + 1))?;
            values.insert(format!("{section}.{}", k.trim()), v);
        }
        Ok(Prereg { values })
    }

    /// The embedded pre-registration.
    pub fn embedded() -> Self {
        Self::parse(EMBEDDED).expect("embedded prereg.toml parses")
    }

    /// Value, panicking if missing.
    pub fn get(&self, key: &str) -> f64 {
        *self
            .values
            .get(key)
            .unwrap_or_else(|| panic!("prereg.toml: missing {key}"))
    }

    /// Integer value.
    pub fn int(&self, key: &str) -> u64 {
        let v = self.get(key);
        assert!(v >= 0.0 && v.fract() == 0.0, "{key} must be a whole number");
        v as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_has_the_registered_values() {
        let p = Prereg::embedded();
        assert_eq!(p.int("device.sector_bytes"), 512);
        assert_eq!(p.int("run.main_seeds_count"), 100_000);
        assert_eq!(p.get("thresholds.bytes_overhead_max"), 0.05);
        assert_eq!(p.get("thresholds.confirmed_corruption_detected_min"), 1.0);
        let sum = p.get("faults.torn_tail")
            + p.get("faults.bit_flip")
            + p.get("faults.misdirected_write");
        assert!((sum - 1.0).abs() < 1e-12);
    }
}
