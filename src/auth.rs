use crate::State;
use axum::http::header::{AUTHORIZATION, HOST};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use jwt_compact::alg::Es256k;
use jwt_compact::{AlgorithmExt, TimeOptions, Token, UntrustedToken};
use log::error;
use nostr::key::XOnlyPublicKey;
use nostr::{Event, Kind};
use secp256k1::PublicKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

const NOSTR_HTTP_AUTH_MAX_AGE_SECS: u64 = 5 * 60;
const NOSTR_HTTP_AUTH_MAX_CLOCK_SKEW_SECS: u64 = 60;

type HttpError = (StatusCode, String);

pub(crate) fn verify_token(
    token: &str,
    state: &State,
) -> Result<Option<String>, (StatusCode, String)> {
    let Some(auth_key) = state.auth_key else {
        return Ok(None);
    };

    let es256k1 = Es256k::<Sha256>::new(state.secp.clone());

    validate_jwt_from_user(token, auth_key, &es256k1)
        .map(Some)
        .map_err(|e| {
            error!("Unauthorized: {e}");
            (StatusCode::UNAUTHORIZED, format!("Unauthorized: {e}"))
        })
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct CustomClaims {
    pub sub: String,
}

fn validate_jwt_from_user(
    token_str: &str,
    auth_key: PublicKey,
    es256k1: &Es256k<Sha256>,
) -> anyhow::Result<String> {
    let untrusted_token = UntrustedToken::new(token_str)?;

    let token: Token<CustomClaims> = es256k1.validator(&auth_key).validate(&untrusted_token)?;

    let time_options = TimeOptions::default();
    token.claims().validate_expiration(&time_options)?;
    token.claims().validate_maturity(&time_options)?;

    let claims = token.claims();

    Ok(claims.custom.sub.clone())
}

pub(crate) fn verify_nostr_http_auth(
    headers: &HeaderMap,
    method: &Method,
    uri: &Uri,
    public_base_url: Option<&str>,
    body: &[u8],
    wallet_service_pubkey: XOnlyPublicKey,
) -> Result<(), HttpError> {
    let auth_header = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                "Unauthorized: Nostr auth required".to_string(),
            )
        })?;
    let encoded_event = auth_header.strip_prefix("Nostr ").ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            "Unauthorized: expected Nostr auth scheme".to_string(),
        )
    })?;
    let event_json = BASE64.decode(encoded_event).map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            format!("Unauthorized: invalid Nostr auth encoding: {e}"),
        )
    })?;
    let event_json = String::from_utf8(event_json).map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            format!("Unauthorized: invalid Nostr auth event JSON: {e}"),
        )
    })?;
    let event = Event::from_json(event_json).map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            format!("Unauthorized: invalid Nostr auth event: {e}"),
        )
    })?;

    if event.kind != Kind::Custom(27235) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized: Nostr auth event must be kind 27235".to_string(),
        ));
    }
    event.verify().map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            format!("Unauthorized: invalid Nostr auth signature: {e}"),
        )
    })?;
    if event.pubkey != wallet_service_pubkey {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized: Nostr auth pubkey does not match wallet_service_pubkey".to_string(),
        ));
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| {
            (
                StatusCode::UNAUTHORIZED,
                format!("Unauthorized: invalid server clock: {e}"),
            )
        })?
        .as_secs();
    let created_at = event.created_at.as_u64();
    if created_at > now.saturating_add(NOSTR_HTTP_AUTH_MAX_CLOCK_SKEW_SECS)
        || now.saturating_sub(created_at) > NOSTR_HTTP_AUTH_MAX_AGE_SECS
    {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized: stale Nostr auth event".to_string(),
        ));
    }

    let expected_url = effective_request_url(headers, uri, public_base_url)?;
    require_auth_tag(&event, "u", &expected_url)?;
    require_auth_tag(&event, "method", method.as_str())?;

    let payload_hash = hex::encode(Sha256::digest(body));
    require_auth_tag(&event, "payload", &payload_hash)?;

    Ok(())
}

fn require_auth_tag(event: &Event, name: &str, expected: &str) -> Result<(), HttpError> {
    let matches = event.tags.iter().any(|tag| {
        let tag = tag.as_vec();
        tag.first().map(String::as_str) == Some(name)
            && tag.get(1).map(String::as_str) == Some(expected)
    });
    if matches {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            format!("Unauthorized: missing or invalid Nostr auth {name} tag"),
        ))
    }
}

fn effective_request_url(
    headers: &HeaderMap,
    uri: &Uri,
    public_base_url: Option<&str>,
) -> Result<String, HttpError> {
    let path = uri
        .path_and_query()
        .map(|path| path.as_str())
        .unwrap_or("/");

    if let Some(public_base_url) = public_base_url {
        return Ok(format!("{}{}", public_base_url.trim_end_matches('/'), path));
    }

    let host = header_value(headers, HOST.as_str()).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            "Unauthorized: host header required".to_string(),
        )
    })?;
    let scheme = if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
        "http"
    } else {
        "https"
    };

    Ok(format!("{scheme}://{host}{path}"))
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Tag};

    fn auth_headers(keys: &Keys, url: &str, method: &str, body: &[u8]) -> HeaderMap {
        let tags = [
            Tag::parse(vec!["u", url]).unwrap(),
            Tag::parse(vec!["method", method]).unwrap(),
            Tag::parse(vec!["payload", &hex::encode(Sha256::digest(body))]).unwrap(),
        ];
        let event = EventBuilder::new(Kind::Custom(27235), "", &tags)
            .to_event(keys)
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            format!("Nostr {}", BASE64.encode(event.as_json()))
                .parse()
                .unwrap(),
        );
        headers
    }

    #[test]
    fn accepts_valid_nostr_http_auth() {
        let keys = Keys::generate();
        let body = br#"{"enabled":true}"#;
        let url = "https://push.example.com/register-nwc-push";
        let headers = auth_headers(&keys, url, "POST", body);

        verify_nostr_http_auth(
            &headers,
            &Method::POST,
            &Uri::from_static("/register-nwc-push"),
            Some("https://push.example.com"),
            body,
            keys.public_key(),
        )
        .unwrap();
    }

    #[test]
    fn rejects_auth_for_a_different_payload() {
        let keys = Keys::generate();
        let url = "https://push.example.com/register-nwc-push";
        let headers = auth_headers(&keys, url, "POST", br#"{"enabled":true}"#);

        let error = verify_nostr_http_auth(
            &headers,
            &Method::POST,
            &Uri::from_static("/register-nwc-push"),
            Some("https://push.example.com"),
            br#"{"enabled":false}"#,
            keys.public_key(),
        )
        .unwrap_err();

        assert_eq!(error.0, StatusCode::UNAUTHORIZED);
        assert!(error.1.contains("payload tag"));
    }

    #[test]
    fn public_base_url_overrides_the_host_header() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, "attacker.example".parse().unwrap());

        assert_eq!(
            effective_request_url(
                &headers,
                &Uri::from_static("/register-nwc-push?test=1"),
                Some("https://push.example.com/"),
            )
            .unwrap(),
            "https://push.example.com/register-nwc-push?test=1"
        );
    }
}
