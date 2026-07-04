#![allow(dead_code)]

use crate::models::nwc_push_registration::NwcPushRegistration;
use anyhow::{Context, Result};
use serde_json::json;
use std::fmt;
use std::time::Duration;

const FCM_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct FcmPushClient {
    http: reqwest::Client,
    project_id: String,
    credentials_path: String,
}

#[derive(Debug)]
pub enum FcmSendError {
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
    NotImplemented,
}

impl FcmSendError {
    pub fn is_permanent(&self) -> bool {
        matches!(self, Self::Permanent { .. })
    }
}

impl fmt::Display for FcmSendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Build(message) => write!(f, "{message}"),
            Self::Request(error) => write!(f, "failed to send FCM wake push: {error}"),
            Self::Permanent { status, body } | Self::Transient { status, body } => {
                write!(f, "FCM wake push failed with {status}: {body}")
            }
            Self::NotImplemented => write!(f, "FCM wake push delivery is not implemented yet"),
        }
    }
}

impl std::error::Error for FcmSendError {}

impl FcmPushClient {
    pub fn from_env() -> Result<Option<Self>> {
        let Some(project_id) = optional_env("FCM_PROJECT_ID") else {
            return Ok(None);
        };
        let credentials_path = std::env::var("FCM_SERVICE_ACCOUNT_PATH")
            .context("FCM_SERVICE_ACCOUNT_PATH must be set")?;
        let http = reqwest::Client::builder()
            .timeout(FCM_REQUEST_TIMEOUT)
            .build()
            .context("failed to build FCM HTTP client")?;

        Ok(Some(Self {
            http,
            project_id,
            credentials_path,
        }))
    }

    pub async fn send_wake(
        &self,
        registration: &NwcPushRegistration,
        relay: &str,
        event_id: &str,
        wallet_service_pubkey: Option<&str>,
    ) -> std::result::Result<(), FcmSendError> {
        let _payload = self.wake_payload(registration, relay, event_id, wallet_service_pubkey);

        Err(FcmSendError::NotImplemented)
    }

    fn wake_payload(
        &self,
        registration: &NwcPushRegistration,
        relay: &str,
        event_id: &str,
        wallet_service_pubkey: Option<&str>,
    ) -> serde_json::Value {
        let wallet_service_pubkey = wallet_service_pubkey.unwrap_or(&registration.tagged);

        json!({
            "message": {
                "token": registration.push_token,
                "android": {
                    "priority": "HIGH"
                },
                "data": {
                    "protocol": "nwc_wake",
                    "version": "v1",
                    "relay": relay,
                    "event_id": event_id,
                    "wallet_service_pubkey": wallet_service_pubkey
                }
            }
        })
    }
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}
