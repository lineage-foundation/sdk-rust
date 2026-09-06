//! HTTP client for the Lineage /v1 API.

use serde::de::DeserializeOwned;

use crate::error::{ApiProblem, Error, Result};

#[derive(Debug, Clone)]
pub struct Hosts {
    pub mempool: String,
    pub storage: String,
    pub miner: String,
}

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    hosts: Hosts,
    api_key: Option<String>,
}

impl Client {
    pub fn new(hosts: Hosts) -> Result<Self> {
        Ok(Client {
            http: reqwest::Client::builder().build()?,
            hosts,
            api_key: None,
        })
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn testnet() -> Result<Client> {
        Client::new(Hosts {
            mempool: "https://mempool.lineage.to".into(),
            storage: "https://storage.lineage.to".into(),
            miner: "https://miner.lineage.to".into(),
        })
    }

    pub(crate) fn hosts(&self) -> &Hosts {
        &self.hosts
    }

    pub(crate) async fn get_json<T: DeserializeOwned>(
        &self,
        base: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let url = format!("{base}{path}");
        let mut req = self.http.get(url).query(query);
        if let Some(key) = &self.api_key {
            req = req.header("x-api-key", key);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if status.is_success() {
            Ok(serde_json::from_slice(&bytes)?)
        } else {
            let problem: ApiProblem = serde_json::from_slice(&bytes).unwrap_or(ApiProblem {
                status: status.as_u16(),
                title: None,
                detail: Some(String::from_utf8_lossy(&bytes).into_owned()),
                request_id: None,
            });
            Err(Error::Api(problem))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client_for(server: &MockServer) -> Client {
        Client::new(Hosts {
            mempool: server.uri(),
            storage: server.uri(),
            miner: server.uri(),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn get_json_decodes_success_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/ping"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let v: serde_json::Value = client.get_json(&client.hosts().mempool.clone(), "/v1/ping", &[]).await.unwrap();
        assert_eq!(v["ok"], true);
    }

    #[tokio::test]
    async fn get_json_maps_problem_to_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/blocks/9999"))
            .respond_with(
                ResponseTemplate::new(404)
                    .insert_header("content-type", "application/problem+json")
                    .set_body_json(serde_json::json!({"status":404,"detail":"No block at that height"})),
            )
            .mount(&server)
            .await;
        let client = client_for(&server);
        let err = client
            .get_json::<serde_json::Value>(&client.hosts().storage.clone(), "/v1/blocks/9999", &[])
            .await
            .unwrap_err();
        match err {
            Error::Api(p) => {
                assert_eq!(p.status, 404);
                assert_eq!(p.detail.as_deref(), Some("No block at that height"));
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_json_sends_api_key_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/debug"))
            .and(header("x-api-key", "secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let client = client_for(&server).with_api_key("secret");
        let _: serde_json::Value = client.get_json(&client.hosts().mempool.clone(), "/v1/debug", &[]).await.unwrap();
        // If the header did not match, wiremock returns 404 and this unwrap panics.
    }
}
