use crate::models::apns_nwc_registration::ApnsNwcRegistration;
use anyhow::{Context, Result};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use nostr::{Event, Tag};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

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
        registration: &ApnsNwcRegistration,
        event: &Event,
        relay: &str,
    ) -> Result<()> {
        self.send_wake(
            registration,
            relay,
            &event.id.to_hex(),
            wallet_service_pubkey(event).as_deref(),
            Some(event.as_json()),
        )
        .await
    }

    pub async fn send_wake(
        &self,
        registration: &ApnsNwcRegistration,
        relay: &str,
        event_id: &str,
        wallet_service_pubkey: Option<&str>,
        event_json: Option<String>,
    ) -> Result<()> {
        let wallet_service_pubkey = wallet_service_pubkey.unwrap_or(&registration.tagged);
        let token = registration
            .device_token
            .chars()
            .filter(|ch| !matches!(ch, '<' | '>' | ' '))
            .collect::<String>();
        let endpoint = match registration.environment.as_str() {
            "production" => format!("https://api.push.apple.com/3/device/{token}"),
            _ => format!("https://api.sandbox.push.apple.com/3/device/{token}"),
        };
        let auth_token = self.auth_token()?;

        let mut payload = json!({
            "aps": {
                "alert": {
                    "title": "Payment request pending",
                    "body": "Open Rebel Wallet to continue."
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
        if let Some(event_json) = event_json {
            payload["nwc_event"] = json!(event_json);
        }

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("bearer {auth_token}"))?,
        );
        headers.insert("apns-topic", HeaderValue::from_str(&registration.bundle_id)?);
        headers.insert("apns-push-type", HeaderValue::from_static("alert"));
        headers.insert("apns-priority", HeaderValue::from_static("10"));

        let response = self
            .http
            .post(endpoint)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("failed to send APNS wake push")?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("APNS wake push failed with {status}: {body}");
        }

        Ok(())
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

fn wallet_service_pubkey(event: &Event) -> Option<String> {
    event.tags.iter().find_map(|tag| {
        if let Tag::PubKey(pubkey, _) = tag {
            Some(hex::encode(pubkey.serialize()))
        } else {
            None
        }
    })
}
