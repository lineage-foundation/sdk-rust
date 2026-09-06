//! Rust SDK for the Lineage network `/v1` API.
//!
//! This crate currently provides a typed async read client. Wallet management and
//! transaction signing arrive in a later release.

pub mod amount;
pub mod client;
pub mod error;
pub mod models;

pub use amount::{Tokens, RAW_PER_LNGX};
pub use client::{Client, Hosts, NodeClass};
pub use error::{ApiProblem, Error, Result};

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        assert_eq!(2 + 2, 4);
    }
}

#[cfg(test)]
mod prelude_tests {
    #[test]
    fn reexports_are_reachable() {
        let _ = crate::Tokens(1);
        fn _takes_hosts(_: crate::Hosts) {}
        let _ = crate::RAW_PER_LNGX;
    }
}
