pub mod amount;
pub mod client;
pub mod error;
pub mod models;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        assert_eq!(2 + 2, 4);
    }
}
