//! Token amounts. On-chain values are raw units; humans think in LNGX.

use serde::{Deserialize, Serialize};

/// Raw token units. 1 LNGX = 72,072,000 raw units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tokens(pub u64);

pub const RAW_PER_LNGX: u64 = 72_072_000;

impl Tokens {
    pub fn from_lngx(lngx: f64) -> Self {
        Tokens((lngx * RAW_PER_LNGX as f64).round() as u64)
    }

    pub fn as_lngx(&self) -> f64 {
        self.0 as f64 / RAW_PER_LNGX as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_lngx_is_expected_raw() {
        assert_eq!(Tokens::from_lngx(10.0), Tokens(720_720_000));
    }

    #[test]
    fn raw_round_trips_to_lngx() {
        assert_eq!(Tokens(720_720_000).as_lngx(), 10.0);
    }

    #[test]
    fn serializes_as_bare_integer() {
        assert_eq!(serde_json::to_string(&Tokens(42)).unwrap(), "42");
    }
}
