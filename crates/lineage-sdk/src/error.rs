//! SDK error types.

use serde::Deserialize;

/// RFC 7807 problem detail, as returned by the node on `application/problem+json`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ApiProblem {
    pub status: u16,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("api error {}: {}", .0.status, .0.detail.clone().unwrap_or_default())]
    Api(ApiProblem),
    #[error("decode error: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("keystore: {0}")]
    Keystore(String),
    #[error("tx: {0}")]
    Tx(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_problem_json() {
        let body = r#"{"type":"about:blank","title":"Not Found","status":404,"detail":"No block at that height","request_id":"abc"}"#;
        let p: ApiProblem = serde_json::from_str(body).unwrap();
        assert_eq!(p.status, 404);
        assert_eq!(p.detail.as_deref(), Some("No block at that height"));
    }

    #[test]
    fn error_display_includes_status_and_detail() {
        let e = Error::Api(ApiProblem { status: 404, title: None, detail: Some("nope".into()), request_id: None });
        assert_eq!(e.to_string(), "api error 404: nope");
    }
}
