use crate::auth::{verify_nostr_http_auth, verify_token};
use crate::models::nwc_invoice_monitor::NwcInvoiceMonitor;
use crate::models::nwc_pubkey::NwcPubkeys;
use crate::models::nwc_push_registration::{NwcPushRegistration, PUSH_SERVICE_APNS};
use crate::routes::{ensure_request_id, handle_anyhow_error, validate_cors, HttpError};
use crate::State;
use axum::body::Bytes;
use axum::headers::authorization::Bearer;
use axum::headers::{Authorization, Origin};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::{Extension, Json, TypedHeader};
use chrono::{Duration, TimeZone, Utc};
use log::info;
use nostr::key::XOnlyPublicKey;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterNwcRequest {
    pub id: Option<String>,
    pub author: XOnlyPublicKey,
    pub tagged: XOnlyPublicKey,
    pub relay: String,
    pub name: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorNwcInvoiceRequest {
    pub id: String,
    pub request_event_id: String,
    pub client_pubkey: XOnlyPublicKey,
    pub wallet_service_pubkey: XOnlyPublicKey,
    pub relay: String,
    pub expires_at: u64,
    pub enabled: bool,
    pub trigger_token_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TriggerNwcInvoiceRequest {
    pub request_event_id: String,
    pub trigger_token: String,
}

async fn refresh_watcher_filter(state: &State) -> anyhow::Result<(usize, usize, usize, bool)> {
    let mut conn = state.db_pool.get()?;
    let mut updated_filter_info = NwcPubkeys::get_filter_info(&mut conn)?;
    updated_filter_info.merge(NwcPushRegistration::get_filter_info(&mut conn)?);
    let counts = (
        updated_filter_info.authors.len(),
        updated_filter_info.tagged.len(),
        updated_filter_info.relays.len(),
    );
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

    Ok((counts.0, counts.1, counts.2, changed))
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
    drop(conn);

    refresh_watcher_filter(state).await?;
    info!("Registered nwc keys!");

    Ok(())
}

pub async fn register_nwc(
    origin: Option<TypedHeader<Origin>>,
    auth: Option<TypedHeader<Authorization<Bearer>>>,
    Extension(state): Extension<State>,
    Json(mut payload): Json<RegisterNwcRequest>,
) -> Result<Json<()>, HttpError> {
    if !state.self_hosted {
        validate_cors(origin)?;
    }

    let auth_id = auth
        .map(|TypedHeader(token)| verify_token(token.token(), &state))
        .transpose()?
        .flatten();
    ensure_request_id(&mut payload.id, auth_id)?;

    match register_nwc_impl(&state, payload).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("register_nwc", e)),
    }
}

async fn register_nwc_push_impl(
    state: &State,
    payload: RegisterNwcPushRequest,
    push_service: String,
) -> anyhow::Result<()> {
    let mut conn = state.db_pool.get()?;
    let id = payload.id.as_deref().expect("must have");
    let enabled = payload.enabled.unwrap_or(true);
    let author = hex::encode(payload.client_pubkey.serialize());
    let tagged = hex::encode(payload.wallet_service_pubkey.serialize());

    let deleted = if enabled {
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
            true,
        )?;
        0
    } else {
        NwcPushRegistration::unregister(
            &mut conn,
            id,
            &push_service,
            &author,
            &tagged,
            &payload.relay,
        )?
    };
    drop(conn);

    let (active_clients, active_wallets, active_relays, changed) =
        refresh_watcher_filter(state).await?;

    if enabled {
        if crate::debug_logging_enabled() {
            println!(
                "NWC push registration updated: operation=register push_service={} registration_id={} client_pubkey={} wallet_service_pubkey={} relay={} active_clients={} active_wallets={} active_relays={} watcher_filter_changed={}",
                push_service,
                id,
                author,
                tagged,
                payload.relay,
                active_clients,
                active_wallets,
                active_relays,
                changed
            );
        }
        info!(
            "Registered {} NWC wake push connection id={} client_pubkey={} wallet_service_pubkey={} relay={} watcher_filter_changed={}",
            push_service, id, author, tagged, payload.relay, changed
        );
    } else {
        if crate::debug_logging_enabled() {
            println!(
                "NWC push registration updated: operation=unregister push_service={} registration_id={} client_pubkey={} wallet_service_pubkey={} relay={} deleted={} active_clients={} active_wallets={} active_relays={} watcher_filter_changed={}",
                push_service,
                id,
                author,
                tagged,
                payload.relay,
                deleted,
                active_clients,
                active_wallets,
                active_relays,
                changed
            );
        }
        info!(
            "Unregistered {} NWC wake push connection id={} client_pubkey={} wallet_service_pubkey={} relay={} deleted={} watcher_filter_changed={}",
            push_service, id, author, tagged, payload.relay, deleted, changed
        );
    }

    Ok(())
}

pub async fn register_nwc_push(
    origin: Option<TypedHeader<Origin>>,
    Extension(state): Extension<State>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<()>, HttpError> {
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
    let push_service = validate_push_request(&payload)?;

    match register_nwc_push_impl(&state, payload, push_service).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("register_nwc_push", e)),
    }
}

pub async fn monitor_nwc_invoice(
    origin: Option<TypedHeader<Origin>>,
    Extension(state): Extension<State>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<()>, HttpError> {
    if !state.self_hosted {
        validate_cors(origin)?;
    }
    let payload: MonitorNwcInvoiceRequest = parse_json_body(&body)?;
    verify_nostr_http_auth(
        &headers,
        &method,
        &uri,
        state.public_base_url.as_deref(),
        &body,
        payload.wallet_service_pubkey,
    )?;
    validate_monitor_request(&payload)?;
    let mut conn = state
        .db_pool
        .get()
        .map_err(|error| handle_anyhow_error("monitor_nwc_invoice", error.into()))?;
    let client = hex::encode(payload.client_pubkey.serialize());
    let wallet = hex::encode(payload.wallet_service_pubkey.serialize());
    let request_event_id = payload.request_event_id.to_ascii_lowercase();
    let trigger_token_hash = payload.trigger_token_hash.to_ascii_lowercase();
    if payload.enabled {
        let registrations = NwcPushRegistration::find_apns_by_nwc(
            &mut conn,
            payload.client_pubkey,
            payload.wallet_service_pubkey,
            &payload.relay,
        )
        .map_err(|error| handle_anyhow_error("monitor_nwc_invoice", error))?;
        if !registrations
            .iter()
            .any(|registration| registration.id == payload.id)
        {
            return Err((
                StatusCode::FORBIDDEN,
                "No active push registration for settlement monitor".to_string(),
            ));
        }
        let expires_at_seconds = i64::try_from(payload.expires_at)
            .map_err(|_| (StatusCode::BAD_REQUEST, "invalid expires_at".to_string()))?;
        let expires_at = Utc
            .timestamp_opt(expires_at_seconds, 0)
            .single()
            .ok_or((StatusCode::BAD_REQUEST, "invalid expires_at".to_string()))?;
        NwcInvoiceMonitor::enable(
            &mut conn,
            &payload.id,
            &request_event_id,
            &client,
            &wallet,
            &payload.relay,
            expires_at,
            &trigger_token_hash,
        )
        .map_err(|error| handle_anyhow_error("monitor_nwc_invoice", error))?;
    } else {
        NwcInvoiceMonitor::disable(
            &mut conn,
            &payload.id,
            &request_event_id,
            &wallet,
            &payload.relay,
        )
        .map_err(|error| handle_anyhow_error("monitor_nwc_invoice", error))?;
    }
    Ok(Json(()))
}

/// Accepts a single-invoice bearer capability from the Bark mailbox hook.
///
/// The endpoint intentionally returns the same status for valid, expired, and
/// unknown monitors so public Nostr event ids cannot be used as an oracle.
pub async fn trigger_nwc_invoice(
    Extension(state): Extension<State>,
    Json(payload): Json<TriggerNwcInvoiceRequest>,
) -> Result<StatusCode, HttpError> {
    if payload.request_event_id.len() != 64
        || !payload
            .request_event_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || payload.trigger_token.len() != 64
        || !payload
            .trigger_token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid settlement trigger".to_string(),
        ));
    }
    let trigger_token_hash = settlement_trigger_token_hash(&payload.trigger_token).ok_or((
        StatusCode::BAD_REQUEST,
        "invalid settlement trigger".to_string(),
    ))?;
    let mut conn = state
        .db_pool
        .get()
        .map_err(|error| handle_anyhow_error("trigger_nwc_invoice", error.into()))?;
    let _ = NwcInvoiceMonitor::signal_settlement(
        &mut conn,
        &payload.request_event_id.to_ascii_lowercase(),
        &trigger_token_hash,
    )
    .map_err(|error| handle_anyhow_error("trigger_nwc_invoice", error))?;
    Ok(StatusCode::ACCEPTED)
}

fn settlement_trigger_token_hash(token_hex: &str) -> Option<String> {
    let token = hex::decode(token_hex).ok()?;
    (token.len() == 32).then(|| hex::encode(Sha256::digest(token)))
}

fn validate_monitor_request(payload: &MonitorNwcInvoiceRequest) -> Result<(), HttpError> {
    if payload.id.trim().is_empty() || payload.id.len() > 2_048 {
        return Err((StatusCode::BAD_REQUEST, "invalid id".to_string()));
    }
    if payload.request_event_id.len() != 64
        || !payload
            .request_event_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid request_event_id".to_string(),
        ));
    }
    if payload.trigger_token_hash.len() != 64
        || !payload
            .trigger_token_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid trigger_token_hash".to_string(),
        ));
    }
    if payload.relay.len() > 2_048
        || !payload.relay.starts_with("wss://")
        || payload.relay.chars().any(char::is_whitespace)
        || payload.relay.contains('#')
        || match payload.relay.strip_prefix("wss://") {
            Some(authority) => authority.is_empty() || authority.contains('@'),
            None => true,
        }
    {
        return Err((StatusCode::BAD_REQUEST, "invalid relay".to_string()));
    }
    if payload.enabled {
        let now = Utc::now().timestamp();
        let expires_at = i64::try_from(payload.expires_at)
            .map_err(|_| (StatusCode::BAD_REQUEST, "invalid expires_at".to_string()))?;
        if expires_at <= now + 5 || expires_at > (Utc::now() + Duration::days(8)).timestamp() {
            return Err((StatusCode::BAD_REQUEST, "invalid expires_at".to_string()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod settlement_tests {
    use super::settlement_trigger_token_hash;

    #[test]
    fn settlement_trigger_hashes_exactly_32_random_bytes() {
        assert_eq!(
            settlement_trigger_token_hash(&"01".repeat(32)),
            Some("72cd6e8422c407fb6d098690f1130b7ded7ec2f7f5e1d30bd9d521f015363793".to_string())
        );
        assert!(settlement_trigger_token_hash("01").is_none());
        assert!(settlement_trigger_token_hash(&"zz".repeat(32)).is_none());
    }
}

fn parse_json_body<T: DeserializeOwned>(body: &[u8]) -> Result<T, HttpError> {
    serde_json::from_slice(body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid JSON request body: {e}"),
        )
    })
}

fn validate_push_request(payload: &RegisterNwcPushRequest) -> Result<String, HttpError> {
    let push_service = payload.push_service.trim().to_ascii_lowercase();
    if push_service != PUSH_SERVICE_APNS {
        return Err((StatusCode::BAD_REQUEST, "invalid push_service".to_string()));
    }

    if payload.enabled.unwrap_or(true) {
        if !matches!(payload.environment.as_str(), "sandbox" | "production") {
            return Err((
                StatusCode::BAD_REQUEST,
                "invalid push environment".to_string(),
            ));
        }
        if payload.push_token.trim().is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "push_token is required".to_string(),
            ));
        }
        if payload.app_id.trim().is_empty() {
            return Err((StatusCode::BAD_REQUEST, "app_id is required".to_string()));
        }
    }

    Ok(push_service)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::Keys;

    fn request() -> RegisterNwcPushRequest {
        let client_keys = Keys::generate();
        let wallet_keys = Keys::generate();
        RegisterNwcPushRequest {
            id: Some("registration".to_string()),
            push_service: " APNS ".to_string(),
            push_token: "token".to_string(),
            app_id: "com.example.wallet".to_string(),
            environment: "sandbox".to_string(),
            client_pubkey: client_keys.public_key(),
            wallet_service_pubkey: wallet_keys.public_key(),
            relay: "wss://relay.example.com".to_string(),
            name: "Example".to_string(),
            enabled: Some(true),
        }
    }

    #[test]
    fn validates_and_normalizes_push_service() {
        assert_eq!(validate_push_request(&request()).unwrap(), "apns");
    }

    #[test]
    fn unregister_does_not_require_push_delivery_fields() {
        let mut request = request();
        request.enabled = Some(false);
        request.push_token.clear();
        request.app_id.clear();
        request.environment.clear();

        assert_eq!(validate_push_request(&request).unwrap(), "apns");
    }

    #[test]
    fn invalid_push_environment_is_a_bad_request() {
        let mut request = request();
        request.environment = "development".to_string();

        assert_eq!(
            validate_push_request(&request).unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
    }

    fn monitor_request() -> MonitorNwcInvoiceRequest {
        MonitorNwcInvoiceRequest {
            id: "install".to_string(),
            request_event_id: "ab".repeat(32),
            client_pubkey: Keys::generate().public_key(),
            wallet_service_pubkey: Keys::generate().public_key(),
            relay: "wss://relay.example.com".to_string(),
            expires_at: (Utc::now() + Duration::hours(1)).timestamp() as u64,
            enabled: true,
            trigger_token_hash: "cd".repeat(32),
        }
    }

    #[test]
    fn settlement_monitor_bounds_public_routing_metadata() {
        assert!(validate_monitor_request(&monitor_request()).is_ok());

        let mut invalid = monitor_request();
        invalid.request_event_id = "not-an-event".to_string();
        assert_eq!(
            validate_monitor_request(&invalid).unwrap_err().0,
            StatusCode::BAD_REQUEST
        );

        let mut invalid = monitor_request();
        invalid.relay = "wss://user@relay.example.com".to_string();
        assert_eq!(
            validate_monitor_request(&invalid).unwrap_err().0,
            StatusCode::BAD_REQUEST
        );

        let mut invalid = monitor_request();
        invalid.expires_at = (Utc::now() + Duration::days(9)).timestamp() as u64;
        assert_eq!(
            validate_monitor_request(&invalid).unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn disabling_monitor_does_not_require_future_expiration() {
        let mut request = monitor_request();
        request.enabled = false;
        request.expires_at = 0;
        assert!(validate_monitor_request(&request).is_ok());
    }
}
