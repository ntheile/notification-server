use crate::auth::verify_token;
use crate::models::nwc_pubkey::NwcPubkeys;
use crate::models::nwc_push_registration::{NwcPushRegistration, PUSH_SERVICE_APNS};
use crate::models::nwc_wake_event::NwcWakeEvent;
use crate::models::subscription_info::SubscriptionInfo;
use crate::{State, ALLOWED_LOCALHOST, ALLOWED_ORIGINS, ALLOWED_SUBDOMAIN};
use axum::body::Bytes;
use axum::headers::authorization::Bearer;
use axum::headers::{Authorization, Origin};
use axum::http::header::{AUTHORIZATION, HOST};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::{Extension, Json, TypedHeader};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use log::{error, info, warn};
use nostr::key::XOnlyPublicKey;
use nostr::{Event, Kind};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use web_push::{ContentEncoding, WebPushClient, WebPushMessageBuilder};
use web_push::{SubscriptionInfo as WebPushSubscriptionInfo, Urgency};

const NWC_WAKE_MAX_EVENT_AGE_SECS: u64 = 10 * 60;
const NWC_WAKE_MAX_CLOCK_SKEW_SECS: u64 = 60;
const NWC_WAKE_EVENT_RETENTION_SECS: u64 =
    NWC_WAKE_MAX_EVENT_AGE_SECS + NWC_WAKE_MAX_CLOCK_SKEW_SECS;
const NOSTR_HTTP_AUTH_MAX_AGE_SECS: u64 = 5 * 60;
const NOSTR_HTTP_AUTH_MAX_CLOCK_SKEW_SECS: u64 = 60;

macro_rules! ensure_id {
    ($payload:ident, $auth_id:expr) => {
        match $payload.id {
            None => {
                // if neither has an id, return an error
                if $auth_id.is_none() {
                    return Err((
                        StatusCode::UNAUTHORIZED,
                        format!("Unauthorized: id required"),
                    ));
                }
                $payload.id = $auth_id
            }
            Some(ref id) => match $auth_id {
                None => (),
                Some(ref auth_id) => {
                    // if both have a id, make sure they match
                    if id != auth_id {
                        return Err((
                            StatusCode::UNAUTHORIZED,
                            format!("Unauthorized: id mismatch"),
                        ));
                    }
                }
            },
        }
    };
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub info: WebPushSubscriptionInfo,
    pub id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterNwcRequest {
    pub id: Option<String>,
    pub author: XOnlyPublicKey,
    pub tagged: XOnlyPublicKey,
    pub relay: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NwcWakeRequest {
    pub protocol: String,
    pub version: String,
    pub relay: String,
    pub event_id: String,
    pub wallet_service_pubkey: XOnlyPublicKey,
    pub client_pubkey: XOnlyPublicKey,
    pub event_created_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NwcWakeResponse {
    Accepted,
    Rejected {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_after: Option<u64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub title: String,
    pub body: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterNwcPushRequest {
    pub id: Option<String>,
    pub push_service: String,
    pub push_token: String,
    pub app_id: String,
    pub environment: String,
    pub client_pubkey: XOnlyPublicKey,
    pub wallet_service_pubkey: XOnlyPublicKey,
    pub relay: String,
    pub name: String,
    pub enabled: Option<bool>,
}

async fn register_impl(state: &State, payload: RegisterRequest) -> anyhow::Result<String> {
    let mut conn = state.db_pool.get()?;
    let id = SubscriptionInfo::register(
        &mut conn,
        payload.id.as_deref().expect("must have"),
        &payload.info.endpoint,
        &payload.info.keys.p256dh,
        &payload.info.keys.auth,
    )?;

    info!("Registered subscription with id: {id}!");

    Ok(id)
}

async fn register_nwc_push_impl(
    state: &State,
    payload: RegisterNwcPushRequest,
) -> anyhow::Result<()> {
    let mut conn = state.db_pool.get()?;
    let id = payload.id.as_deref().expect("must have");
    let push_service = payload.push_service.trim().to_ascii_lowercase();
    validate_push_service(&push_service)?;
    validate_push_environment(&payload.environment)?;
    if payload.push_token.trim().is_empty() {
        anyhow::bail!("push_token is required");
    }
    if payload.app_id.trim().is_empty() {
        anyhow::bail!("app_id is required");
    }

    let author = hex::encode(payload.client_pubkey.serialize());
    let tagged = hex::encode(payload.wallet_service_pubkey.serialize());
    let enabled = payload.enabled.unwrap_or(true);

    NwcPushRegistration::register(
        &mut conn,
        id,
        &push_service,
        &payload.push_token,
        &payload.app_id,
        &payload.environment,
        &author,
        &tagged,
        &payload.relay,
        &payload.name,
        enabled,
    )?;

    let mut updated_filter_info = NwcPubkeys::get_filter_info(&mut conn)?;
    updated_filter_info.merge(NwcPushRegistration::get_filter_info(&mut conn)?);
    drop(conn);

    let filter_info = state.channel.lock().await;
    let changed = filter_info.send_if_modified(|current| {
        if *current == updated_filter_info {
            false
        } else {
            *current = updated_filter_info;
            true
        }
    });

    info!(
        "Registered {} NWC wake push connection id={} client_pubkey={} wallet_service_pubkey={} relay={} enabled={} watcher_filter_changed={}",
        push_service, id, author, tagged, payload.relay, enabled, changed
    );

    Ok(())
}

pub async fn register(
    origin: Option<TypedHeader<Origin>>,
    auth: Option<TypedHeader<Authorization<Bearer>>>,
    Extension(state): Extension<State>,
    Json(mut payload): Json<RegisterRequest>,
) -> Result<Json<String>, (StatusCode, String)> {
    if !state.self_hosted {
        validate_cors(origin)?;
    }

    let auth_id = auth
        .map(|TypedHeader(token)| verify_token(token.token(), &state))
        .transpose()?
        .flatten();

    ensure_id!(payload, auth_id);

    match register_impl(&state, payload).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("register", e)),
    }
}

pub async fn register_nwc_push(
    origin: Option<TypedHeader<Origin>>,
    Extension(state): Extension<State>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<()>, (StatusCode, String)> {
    if !state.self_hosted {
        validate_cors(origin)?;
    }

    let payload: RegisterNwcPushRequest = parse_json_body(&body)?;
    verify_nostr_http_auth(
        &headers,
        &method,
        &uri,
        state.public_base_url.as_deref(),
        &body,
        payload.wallet_service_pubkey,
    )?;
    if payload.id.as_deref().unwrap_or("").trim().is_empty() {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized: id required".to_string(),
        ));
    }

    match register_nwc_push_impl(&state, payload).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("register_nwc_push", e)),
    }
}

fn parse_json_body<T: DeserializeOwned>(body: &[u8]) -> Result<T, (StatusCode, String)> {
    serde_json::from_slice(body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid JSON request body: {e}"),
        )
    })
}

fn verify_nostr_http_auth(
    headers: &HeaderMap,
    method: &Method,
    uri: &Uri,
    public_base_url: Option<&str>,
    body: &[u8],
    wallet_service_pubkey: XOnlyPublicKey,
) -> Result<(), (StatusCode, String)> {
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

fn require_auth_tag(event: &Event, name: &str, expected: &str) -> Result<(), (StatusCode, String)> {
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
) -> Result<String, (StatusCode, String)> {
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

async fn register_nwc_impl(state: &State, payload: RegisterNwcRequest) -> anyhow::Result<()> {
    let mut conn = state.db_pool.get()?;
    let author = hex::encode(payload.author.serialize());
    let tagged = hex::encode(payload.tagged.serialize());
    NwcPubkeys::register(
        &mut conn,
        payload.id.as_deref().expect("must have"),
        &author,
        &tagged,
        &payload.relay,
        &payload.name,
    )?;

    // notify new nwc keys
    let filter_info = state.channel.lock().await;
    filter_info.send_if_modified(|current| {
        let author = if current.authors.contains(&author) {
            false
        } else {
            current.authors.push(author);
            true
        };

        let tagged = if current.tagged.contains(&payload.tagged) {
            false
        } else {
            current.tagged.push(payload.tagged);
            true
        };

        let relay = if current.relays.contains(&payload.relay) {
            false
        } else {
            current.relays.push(payload.relay);
            true
        };

        author || tagged || relay
    });

    info!("Registered nwc keys!");

    Ok(())
}

pub async fn register_nwc(
    origin: Option<TypedHeader<Origin>>,
    auth: Option<TypedHeader<Authorization<Bearer>>>,
    Extension(state): Extension<State>,
    Json(mut payload): Json<RegisterNwcRequest>,
) -> Result<Json<()>, (StatusCode, String)> {
    if !state.self_hosted {
        validate_cors(origin)?;
    }

    let auth_id = auth
        .map(|TypedHeader(token)| verify_token(token.token(), &state))
        .transpose()?
        .flatten();

    ensure_id!(payload, auth_id);

    match register_nwc_impl(&state, payload).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("register_nwc", e)),
    }
}

async fn wake_nwc_impl(state: &State, payload: NwcWakeRequest) -> anyhow::Result<NwcWakeResponse> {
    if payload.protocol != "nwc_wake" {
        return Ok(NwcWakeResponse::Rejected {
            code: "not_allowed".to_string(),
            message: "invalid protocol".to_string(),
            retry_after: None,
        });
    }
    if payload.version != "v1" {
        return Ok(NwcWakeResponse::Rejected {
            code: "not_allowed".to_string(),
            message: "unsupported version".to_string(),
            retry_after: None,
        });
    }
    if !valid_nostr_event_id(&payload.event_id) {
        return Ok(NwcWakeResponse::Rejected {
            code: "not_allowed".to_string(),
            message: "invalid event_id".to_string(),
            retry_after: None,
        });
    }
    if let Some(rejected) = validate_wake_event_freshness(payload.event_created_at)? {
        return Ok(rejected);
    }

    let Some(apns_client) = state.apns_client.clone() else {
        return Ok(NwcWakeResponse::Rejected {
            code: "push_unavailable".to_string(),
            message: "APNS is not configured".to_string(),
            retry_after: None,
        });
    };

    let registrations = {
        let mut conn = state.db_pool.get()?;
        let registrations = NwcPushRegistration::find_apns_by_nwc(
            &mut conn,
            payload.client_pubkey,
            payload.wallet_service_pubkey,
            &payload.relay,
        )?;

        if registrations.is_empty() {
            return Ok(NwcWakeResponse::Rejected {
                code: "unknown_connection".to_string(),
                message: "no registered wake connection matched this request".to_string(),
                retry_after: None,
            });
        }

        NwcWakeEvent::prune_older_than(&mut conn, NWC_WAKE_EVENT_RETENTION_SECS)?;
        if NwcWakeEvent::exists(&mut conn, &payload.event_id)? {
            return Ok(NwcWakeResponse::Rejected {
                code: "replay".to_string(),
                message: "wake event was already processed".to_string(),
                retry_after: None,
            });
        }

        registrations
    };

    let wallet_service_pubkey = hex::encode(payload.wallet_service_pubkey.serialize());
    let mut sent_count = 0usize;
    let mut transient_failure_count = 0usize;
    for registration in &registrations {
        match apns_client
            .send_wake(
                registration,
                &payload.relay,
                &payload.event_id,
                Some(&wallet_service_pubkey),
            )
            .await
        {
            Ok(()) => sent_count += 1,
            Err(err) => {
                warn!(
                    "Failed to send APNS nwc_wake push for event {} to {}: {}",
                    payload.event_id, registration.id, err
                );
                if err.is_permanent() {
                    let mut conn = state.db_pool.get()?;
                    NwcPushRegistration::disable(&mut conn, registration)?;
                    info!(
                        "Disabled stale APNS NWC registration id={} author={} tagged={} relay={}",
                        registration.id,
                        registration.author,
                        registration.tagged,
                        registration.relay
                    );
                } else {
                    transient_failure_count += 1;
                }
            }
        }
    }

    if sent_count == 0 {
        return Ok(NwcWakeResponse::Rejected {
            code: "push_failed".to_string(),
            message: "wake push could not be delivered".to_string(),
            retry_after: if transient_failure_count > 0 {
                Some(30)
            } else {
                None
            },
        });
    }

    let recorded = {
        let mut conn = state.db_pool.get()?;
        NwcWakeEvent::record_once(&mut conn, &payload.event_id, payload.event_created_at)?
    };
    if !recorded {
        warn!(
            "NWC wake event {} was delivered but was already recorded",
            payload.event_id
        );
    }

    Ok(NwcWakeResponse::Accepted)
}

fn valid_nostr_event_id(event_id: &str) -> bool {
    event_id.len() == 64
        && hex::decode(event_id)
            .map(|bytes| bytes.len() == 32)
            .unwrap_or(false)
}

fn validate_wake_event_freshness(
    event_created_at: Option<u64>,
) -> anyhow::Result<Option<NwcWakeResponse>> {
    let Some(event_created_at) = event_created_at else {
        return Ok(None);
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    if event_created_at > now.saturating_add(NWC_WAKE_MAX_CLOCK_SKEW_SECS) {
        return Ok(Some(NwcWakeResponse::Rejected {
            code: "stale_event".to_string(),
            message: "wake event is from the future".to_string(),
            retry_after: None,
        }));
    }
    if now.saturating_sub(event_created_at) > NWC_WAKE_MAX_EVENT_AGE_SECS {
        return Ok(Some(NwcWakeResponse::Rejected {
            code: "stale_event".to_string(),
            message: "wake event is too old".to_string(),
            retry_after: None,
        }));
    }
    Ok(None)
}

fn validate_push_service(push_service: &str) -> anyhow::Result<()> {
    match push_service {
        PUSH_SERVICE_APNS => Ok(()),
        _ => anyhow::bail!("invalid push_service"),
    }
}

fn validate_push_environment(environment: &str) -> anyhow::Result<()> {
    match environment {
        "sandbox" | "production" => Ok(()),
        _ => anyhow::bail!("invalid push environment"),
    }
}

pub async fn wake_nwc(
    origin: Option<TypedHeader<Origin>>,
    Extension(state): Extension<State>,
    Json(payload): Json<NwcWakeRequest>,
) -> Result<Json<NwcWakeResponse>, (StatusCode, String)> {
    if !state.self_hosted {
        validate_cors(origin)?;
    }

    match wake_nwc_impl(&state, payload).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("wake_nwc", e)),
    }
}

async fn broadcast_individual(
    state: &State,
    subscription_info: WebPushSubscriptionInfo,
    notification: &Notification,
) -> anyhow::Result<()> {
    let sig_builder = state
        .sig_builder
        .clone()
        .add_sub_info(&subscription_info)
        .build()?;

    // Now add payload and encrypt.
    let mut builder = WebPushMessageBuilder::new(&subscription_info);
    let content = json!(notification).to_string();
    builder.set_payload(ContentEncoding::Aes128Gcm, content.as_bytes());
    builder.set_vapid_signature(sig_builder);
    builder.set_urgency(Urgency::High);

    // Finally, send the notification!
    state.client.send(builder.build()?).await?;

    Ok(())
}

async fn broadcast_impl(state: &State, notification: Notification) -> anyhow::Result<()> {
    let mut conn = state.db_pool.get()?;
    let all = SubscriptionInfo::get_all(&mut conn)?;

    // send in parallel
    let mut futures = Vec::with_capacity(all.len());
    for item in all {
        let subscription_info = item.into_web_push();
        let fut = broadcast_individual(state, subscription_info, &notification);
        futures.push(fut);
    }
    // join all futures
    futures::future::try_join_all(futures).await?;

    Ok(())
}

// todo add admin auth
pub async fn broadcast(
    Extension(state): Extension<State>,
    Json(notification): Json<Notification>,
) -> Result<Json<()>, (StatusCode, String)> {
    match broadcast_impl(&state, notification).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("broadcast", e)),
    }
}

pub async fn health_check() -> Result<Json<()>, (StatusCode, String)> {
    Ok(Json(()))
}

pub fn valid_origin(origin: &str) -> bool {
    ALLOWED_ORIGINS.contains(&origin)
        || origin.ends_with(ALLOWED_SUBDOMAIN)
        || origin.starts_with(ALLOWED_LOCALHOST)
}

pub fn validate_cors(origin: Option<TypedHeader<Origin>>) -> Result<(), (StatusCode, String)> {
    if let Some(TypedHeader(origin)) = origin {
        if origin.is_null() {
            return Ok(());
        }

        let origin_str = origin.to_string();
        if valid_origin(&origin_str) {
            return Ok(());
        } else {
            // The origin is not in the allowed list, block the request
            return Err((StatusCode::NOT_FOUND, String::new()));
        }
    }

    Ok(())
}

pub(crate) fn handle_anyhow_error(function: &str, err: anyhow::Error) -> (StatusCode, String) {
    error!("Error in {function}: {err:?}");
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{err}"))
}
