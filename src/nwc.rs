use crate::auth::{verify_nostr_http_auth, verify_token};
use crate::models::nwc_pubkey::NwcPubkeys;
use crate::models::nwc_push_registration::{NwcPushRegistration, PUSH_SERVICE_APNS};
use crate::routes::{ensure_request_id, handle_anyhow_error, validate_cors, HttpError};
use crate::State;
use axum::body::Bytes;
use axum::headers::authorization::Bearer;
use axum::headers::{Authorization, Origin};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::{Extension, Json, TypedHeader};
use log::info;
use nostr::key::XOnlyPublicKey;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

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
}
