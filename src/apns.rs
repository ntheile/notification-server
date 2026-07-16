use crate::models::nwc_push_registration::NwcPushRegistration;
use anyhow::{Context, Result};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use log::warn;
use nostr::{Event, Tag};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const APNS_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_APNS_PAYLOAD_BYTES: usize = 4096;

#[derive(Debug)]
pub struct ApnsSendReceipt {
    pub apns_id: Option<String>,
    pub payload_bytes: usize,
    pub embedded_event: bool,
}

#[derive(Clone)]
pub struct ApnsPushClient {
    http: reqwest::Client,
    team_id: String,
    key_id: String,
    encoding_key: Arc<EncodingKey>,
    auth_token: Arc<Mutex<Option<CachedApnsToken>>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApnsClaims {
    iss: String,
    iat: u64,
}

#[derive(Debug, Clone)]
struct CachedApnsToken {
    token: String,
    issued_at: u64,
}

#[derive(Debug)]
pub enum ApnsSendError {
    Build(String),
    Request(reqwest::Error),
    Permanent {
        status: reqwest::StatusCode,
        body: String,
    },
    Transient {
        status: reqwest::StatusCode,
        body: String,
    },
}

impl ApnsSendError {
    pub fn is_permanent(&self) -> bool {
        matches!(self, Self::Permanent { .. })
    }
}

impl fmt::Display for ApnsSendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Build(message) => write!(f, "{message}"),
            Self::Request(error) => write!(f, "failed to send APNS wake push: {error}"),
            Self::Permanent { status, body } | Self::Transient { status, body } => {
                write!(f, "APNS wake push failed with {status}: {body}")
            }
        }
    }
}

impl std::error::Error for ApnsSendError {}

impl ApnsPushClient {
    pub fn from_env() -> Result<Option<Self>> {
        let Some(team_id) = optional_env("APNS_TEAM_ID") else {
            return Ok(None);
        };
        let key_id = std::env::var("APNS_KEY_ID").context("APNS_KEY_ID must be set")?;
        let private_key = match optional_env("APNS_PRIVATE_KEY") {
            Some(key) => key,
            None => {
                let path = std::env::var("APNS_PRIVATE_KEY_PATH")
                    .context("APNS_PRIVATE_KEY or APNS_PRIVATE_KEY_PATH must be set")?;
                std::fs::read_to_string(path).context("failed to read APNS private key")?
            }
        };
        let encoding_key =
            EncodingKey::from_ec_pem(private_key.as_bytes()).context("invalid APNS .p8 key")?;
        let http = reqwest::Client::builder()
            .timeout(APNS_REQUEST_TIMEOUT)
            .build()
            .context("failed to build APNS HTTP client")?;

        Ok(Some(Self {
            http,
            team_id,
            key_id,
            encoding_key: Arc::new(encoding_key),
            auth_token: Arc::new(Mutex::new(None)),
        }))
    }

    pub async fn send_wake_for_event(
        &self,
        registration: &NwcPushRegistration,
        event: &Event,
        relay: &str,
    ) -> std::result::Result<ApnsSendReceipt, ApnsSendError> {
        let event_json = serde_json::to_string(event)
            .map_err(|e| ApnsSendError::Build(format!("failed to serialize NWC event: {e}")))?;

        self.send_wake_inner(
            registration,
            relay,
            &event.id.to_hex(),
            wallet_service_pubkey(event).as_deref(),
            Some(event_json),
        )
        .await
    }

    pub async fn send_wake(
        &self,
        registration: &NwcPushRegistration,
        relay: &str,
        event_id: &str,
        wallet_service_pubkey: Option<&str>,
    ) -> std::result::Result<ApnsSendReceipt, ApnsSendError> {
        self.send_wake_inner(registration, relay, event_id, wallet_service_pubkey, None)
            .await
    }

    async fn send_wake_inner(
        &self,
        registration: &NwcPushRegistration,
        relay: &str,
        event_id: &str,
        wallet_service_pubkey: Option<&str>,
        nwc_event: Option<String>,
    ) -> std::result::Result<ApnsSendReceipt, ApnsSendError> {
        let wallet_service_pubkey = wallet_service_pubkey.unwrap_or(&registration.tagged);
        let token = registration
            .push_token
            .chars()
            .filter(|ch| !matches!(ch, '<' | '>' | ' '))
            .collect::<String>();
        let endpoint = match registration.environment.as_str() {
            "production" => format!("https://api.push.apple.com/3/device/{token}"),
            _ => format!("https://api.sandbox.push.apple.com/3/device/{token}"),
        };
        let auth_token = self
            .auth_token()
            .map_err(|e| ApnsSendError::Build(format!("failed to create APNS auth token: {e}")))?;

        let mut embedded_event = nwc_event.is_some();
        let mut payload =
            wake_payload(relay, event_id, wallet_service_pubkey, nwc_event.as_deref());
        if nwc_event.is_some() && payload_size(&payload)? > MAX_APNS_PAYLOAD_BYTES {
            warn!(
                "Omitting embedded NWC event from APNS wake payload for event {} because it exceeds {} bytes",
                event_id, MAX_APNS_PAYLOAD_BYTES
            );
            payload = wake_payload(relay, event_id, wallet_service_pubkey, None);
            embedded_event = false;
        }
        let size = payload_size(&payload)?;
        if size > MAX_APNS_PAYLOAD_BYTES {
            return Err(ApnsSendError::Build(format!(
                "APNS wake payload is {size} bytes, exceeding {MAX_APNS_PAYLOAD_BYTES} bytes"
            )));
        }

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("bearer {auth_token}"))
                .map_err(|e| ApnsSendError::Build(format!("invalid APNS auth header: {e}")))?,
        );
        headers.insert(
            "apns-topic",
            HeaderValue::from_str(&registration.app_id)
                .map_err(|e| ApnsSendError::Build(format!("invalid APNS topic header: {e}")))?,
        );
        headers.insert("apns-push-type", HeaderValue::from_static("alert"));
        headers.insert("apns-priority", HeaderValue::from_static("10"));
        let response = self
            .http
            .post(endpoint)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .map_err(ApnsSendError::Request)?;
        let status = response.status();
        let apns_id = response
            .headers()
            .get("apns-id")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if is_permanent_apns_failure(status) {
                return Err(ApnsSendError::Permanent { status, body });
            }
            return Err(ApnsSendError::Transient { status, body });
        }

        Ok(ApnsSendReceipt {
            apns_id,
            payload_bytes: size,
            embedded_event,
        })
    }

    fn auth_token(&self) -> Result<String> {
        let iat = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before unix epoch")?
            .as_secs();

        let mut cached = self
            .auth_token
            .lock()
            .map_err(|_| anyhow::anyhow!("APNS provider token cache was poisoned"))?;
        if let Some(cached) = cached.as_ref() {
            if iat.saturating_sub(cached.issued_at) < 50 * 60 {
                return Ok(cached.token.clone());
            }
        }

        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.key_id.clone());
        let claims = ApnsClaims {
            iss: self.team_id.clone(),
            iat,
        };

        let token = encode(&header, &claims, &self.encoding_key)
            .context("failed to sign APNS provider token")?;
        *cached = Some(CachedApnsToken {
            token: token.clone(),
            issued_at: iat,
        });

        Ok(token)
    }
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn wake_payload(
    relay: &str,
    event_id: &str,
    wallet_service_pubkey: &str,
    nwc_event: Option<&str>,
) -> serde_json::Value {
    let mut payload = json!({
        "aps": {
            "alert": {
                "title": "Nostr Connect",
                "body": "Received 1 Event"
            },
            "mutable-content": 1,
            "content-available": 1
        },
        "protocol": "nwc_wake",
        "version": "v1",
        "relay": relay,
        "event_id": event_id,
        "wallet_service_pubkey": wallet_service_pubkey
    });
    if let Some(nwc_event) = nwc_event {
        payload["nwc_event"] = json!(nwc_event);
    }
    payload
}

fn payload_size(payload: &serde_json::Value) -> std::result::Result<usize, ApnsSendError> {
    serde_json::to_vec(payload)
        .map(|bytes| bytes.len())
        .map_err(|e| ApnsSendError::Build(format!("failed to serialize APNS wake payload: {e}")))
}

fn wallet_service_pubkey(event: &Event) -> Option<String> {
    event.tags.iter().find_map(|tag| {
        if let Tag::PubKey(pubkey, _) = tag {
            Some(hex::encode(pubkey.serialize()))
        } else {
            None
        }
    })
}

fn is_permanent_apns_failure(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::BAD_REQUEST
            | reqwest::StatusCode::FORBIDDEN
            | reqwest::StatusCode::PAYLOAD_TOO_LARGE
            | reqwest::StatusCode::GONE
    )
}
