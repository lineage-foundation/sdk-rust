//! Client for a valence mailbox host: the plaintext message-relay service
//! two-way payment counterparties use to exchange DRUID trade offers,
//! acceptances, and rejections. Messages are sent and stored in plaintext --
//! valence provides delivery and mailbox scoping, not confidentiality.
//!
//! Every request is authenticated by signing the target mailbox address's
//! raw bytes with the caller's own keypair (see [`ValenceClient::post`] and
//! friends), matching sdk-go's `valenceAuthHeaders` / sdk-php's
//! `ValenceClient::authHeaders` / sdk-js's `generateVerificationHeaders`
//! byte-for-byte.

use std::collections::HashMap;

use tw_chain::crypto::sign_ed25519 as sign;

use crate::error::{ApiProblem, Error, Result};
use crate::models::Pending2WTxDetails;

/// The mailbox endpoint on a valence host: every mailbox operation (post an
/// offer, read a mailbox, delete a settled entry) lives under this single
/// path, matching sdk-js's `IAPIRoute.ValenceSet` / `ValenceGet` /
/// `ValenceDel` (all `/messages`, DELETE additionally suffixed with
/// `/{id}`).
const MESSAGES_PATH: &str = "/messages";

/// A client for a single valence mailbox host.
#[derive(Debug, Clone)]
pub struct ValenceClient {
    http: reqwest::Client,
    host: String,
}

impl ValenceClient {
    pub fn new(host: impl Into<String>) -> Result<Self> {
        Ok(ValenceClient {
            http: reqwest::Client::builder().build()?,
            host: host.into(),
        })
    }

    /// Sets the `address`/`public_key`/`signature` headers a valence request
    /// must carry: `address` is the target mailbox's address (hex),
    /// `public_key` is the caller's own public key (hex), and `signature` is
    /// a detached ed25519 signature over the mailbox address string's raw
    /// UTF-8 bytes (unhashed, PLAINTEXT). The signing keypair need not be the
    /// mailbox address's own keypair: valence messages may be posted into
    /// (or deleted from) a counterparty's mailbox, signed by the sender's
    /// own key, as proof of a validly-held keypair rather than of mailbox
    /// ownership.
    fn sign_headers(
        req: reqwest::RequestBuilder,
        address: &str,
        public_key: &sign::PublicKey,
        secret_key: &sign::SecretKey,
    ) -> reqwest::RequestBuilder {
        let signature = sign::sign_detached(address.as_bytes(), secret_key);
        req.header("address", address)
            .header("public_key", hex::encode(public_key.as_ref()))
            .header("signature", hex::encode(signature.as_ref()))
    }

    async fn handle_response<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if status.is_success() {
            Ok(serde_json::from_slice(&bytes)?)
        } else {
            Err(Error::Api(problem_from(status, &bytes)))
        }
    }

    async fn handle_empty(resp: reqwest::Response) -> Result<()> {
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let bytes = resp.bytes().await?;
            Err(Error::Api(problem_from(status, &bytes)))
        }
    }

    /// `POST /messages`: places (or overwrites) the plaintext offer/status
    /// `details` under `address`'s mailbox (`details.druid` is the mailbox
    /// entry's id), signed by `public_key`/`secret_key`. Mirrors sdk-go's
    /// `ValenceClient.Post` / sdk-js's `POST /messages`
    /// (`IAPIRoute.ValenceSet`) call in `make2WayPayment` /
    /// `handle2WTxResponse`.
    pub async fn post(
        &self,
        address: &str,
        public_key: &sign::PublicKey,
        secret_key: &sign::SecretKey,
        details: &Pending2WTxDetails,
    ) -> Result<()> {
        let body = serde_json::json!({ "id": details.druid, "data": details });
        let url = format!("{}{MESSAGES_PATH}", self.host);
        let req = Self::sign_headers(
            self.http.post(url).json(&body),
            address,
            public_key,
            secret_key,
        );
        Self::handle_empty(req.send().await?).await
    }

    /// `GET /messages`: returns the full contents of `address`'s mailbox, a
    /// map of DRUID to the stored [`Pending2WTxDetails`] payload directly
    /// (not wrapped), signed by `public_key`/`secret_key`. Mirrors sdk-go's
    /// `ValenceClient.Get` / sdk-js's `GET /messages`
    /// (`IAPIRoute.ValenceGet`) call in `fetchPending2WayPayment`.
    pub async fn get(
        &self,
        address: &str,
        public_key: &sign::PublicKey,
        secret_key: &sign::SecretKey,
    ) -> Result<HashMap<String, Pending2WTxDetails>> {
        let url = format!("{}{MESSAGES_PATH}", self.host);
        let req = Self::sign_headers(self.http.get(url), address, public_key, secret_key);
        Self::handle_response(req.send().await?).await
    }

    /// `DELETE /messages/{druid}`: removes the mailbox entry identified by
    /// `druid` from `address`'s mailbox, signed by `public_key`/`secret_key`.
    /// Mirrors sdk-go's `ValenceClient.Delete` / sdk-js's
    /// `DELETE /messages/{id}` (`IAPIRoute.ValenceDel`) call in
    /// `fetchPending2WayPayment`.
    pub async fn delete(
        &self,
        druid: &str,
        address: &str,
        public_key: &sign::PublicKey,
        secret_key: &sign::SecretKey,
    ) -> Result<()> {
        let url = format!("{}{MESSAGES_PATH}/{druid}", self.host);
        let req = Self::sign_headers(self.http.delete(url), address, public_key, secret_key);
        Self::handle_empty(req.send().await?).await
    }
}

fn problem_from(status: reqwest::StatusCode, bytes: &[u8]) -> ApiProblem {
    serde_json::from_slice(bytes).unwrap_or(ApiProblem {
        status: status.as_u16(),
        title: None,
        detail: Some(String::from_utf8_lossy(bytes).into_owned()),
        request_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tw_chain::crypto::sign_ed25519::gen_keypair;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sample_details() -> Pending2WTxDetails {
        Pending2WTxDetails {
            druid: "DRUID0xabc".into(),
            sender_expectation: crate::models::DruidExpectation {
                from: String::new(),
                to: "addr-a".into(),
                asset: tw_chain::primitives::asset::Asset::Token(
                    tw_chain::primitives::asset::TokenAmount(1),
                ),
            },
            receiver_expectation: crate::models::DruidExpectation {
                from: String::new(),
                to: "addr-b".into(),
                asset: tw_chain::primitives::asset::Asset::Token(
                    tw_chain::primitives::asset::TokenAmount(2),
                ),
            },
            status: crate::models::Pending2WTxStatus::Pending,
            mempool_host: "https://mempool.example".into(),
        }
    }

    #[tokio::test]
    async fn post_signs_the_raw_mailbox_address_and_sends_the_plaintext_body() {
        let (pk, sk) = gen_keypair();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/messages"))
            .and(header("address", "mailbox-address"))
            .and(header("public_key", hex::encode(pk.as_ref())))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let valence = ValenceClient::new(server.uri()).unwrap();
        let details = sample_details();
        valence
            .post("mailbox-address", &pk, &sk, &details)
            .await
            .unwrap();

        let signature_hex = server.received_requests().await.unwrap()[0]
            .headers
            .get("signature")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let signature_bytes = hex::decode(signature_hex).unwrap();
        let signature = sign::Signature::from_slice(&signature_bytes).unwrap();
        assert!(sign::verify_detached(&signature, b"mailbox-address", &pk));
    }

    #[tokio::test]
    async fn get_decodes_mailbox_map() {
        let (pk, sk) = gen_keypair();
        let server = MockServer::start().await;
        let details = sample_details();
        Mock::given(method("GET"))
            .and(path("/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(HashMap::from([(details.druid.clone(), details.clone())])),
            )
            .mount(&server)
            .await;

        let valence = ValenceClient::new(server.uri()).unwrap();
        let entries = valence.get("mailbox-address", &pk, &sk).await.unwrap();
        assert_eq!(entries.get(&details.druid), Some(&details));
    }

    #[tokio::test]
    async fn delete_hits_the_druid_scoped_path() {
        let (pk, sk) = gen_keypair();
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/messages/DRUID0xabc"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let valence = ValenceClient::new(server.uri()).unwrap();
        valence
            .delete("DRUID0xabc", "mailbox-address", &pk, &sk)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn non_success_status_maps_to_api_error() {
        let (pk, sk) = gen_keypair();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/messages"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let valence = ValenceClient::new(server.uri()).unwrap();
        let err = valence.get("mailbox-address", &pk, &sk).await.unwrap_err();
        match err {
            Error::Api(p) => assert_eq!(p.status, 500),
            other => panic!("expected Api error, got {other:?}"),
        }
    }
}
