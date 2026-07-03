use crate::auth::verify_token;
use crate::models::apns_nwc_registration::ApnsNwcRegistration;
use crate::models::nwc_pubkey::NwcPubkeys;
use crate::models::subscription_info::SubscriptionInfo;
use crate::{State, ALLOWED_LOCALHOST, ALLOWED_ORIGINS, ALLOWED_SUBDOMAIN};
use axum::headers::authorization::Bearer;
use axum::headers::{Authorization, Origin};
use axum::http::StatusCode;
use axum::{Extension, Json, TypedHeader};
use log::{error, info};
use nostr::key::XOnlyPublicKey;
use serde::{Deserialize, Serialize};
use serde_json::json;
use web_push::{ContentEncoding, WebPushClient, WebPushMessageBuilder};
use web_push::{SubscriptionInfo as WebPushSubscriptionInfo, Urgency};

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
pub struct RegisterApnsNwcRequest {
    pub id: Option<String>,
    pub device_token: String,
    pub bundle_id: Option<String>,
    pub environment: Option<String>,
    pub author: XOnlyPublicKey,
    pub tagged: XOnlyPublicKey,
    pub relay: String,
    pub name: String,
    pub enabled: Option<bool>,
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

async fn register_apns_nwc_impl(
    state: &State,
    payload: RegisterApnsNwcRequest,
) -> anyhow::Result<()> {
    let mut conn = state.db_pool.get()?;
    let id = payload.id.as_deref().expect("must have");
    let author = hex::encode(payload.author.serialize());
    let tagged = hex::encode(payload.tagged.serialize());
    let bundle_id = payload
        .bundle_id
        .as_deref()
        .unwrap_or("com.nicktee.rebelwallet");
    let environment = payload.environment.as_deref().unwrap_or("sandbox");
    let enabled = payload.enabled.unwrap_or(true);

    ApnsNwcRegistration::register(
        &mut conn,
        id,
        &payload.device_token,
        bundle_id,
        environment,
        &author,
        &tagged,
        &payload.relay,
        &payload.name,
        enabled,
    )?;

    let filter_info = state.channel.lock().await;
    filter_info.send_if_modified(|current| {
        let author_changed = if current.authors.contains(&author) {
            false
        } else {
            current.authors.push(author);
            true
        };

        let tagged_changed = if current.tagged.contains(&payload.tagged) {
            false
        } else {
            current.tagged.push(payload.tagged);
            true
        };

        let relay_changed = if current.relays.contains(&payload.relay) {
            false
        } else {
            current.relays.push(payload.relay);
            true
        };

        author_changed || tagged_changed || relay_changed
    });

    info!("Registered APNS NWC wake connection!");

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

pub async fn register_apns_nwc(
    origin: Option<TypedHeader<Origin>>,
    auth: Option<TypedHeader<Authorization<Bearer>>>,
    Extension(state): Extension<State>,
    Json(mut payload): Json<RegisterApnsNwcRequest>,
) -> Result<Json<()>, (StatusCode, String)> {
    if !state.self_hosted {
        validate_cors(origin)?;
    }

    let auth_id = auth
        .map(|TypedHeader(token)| verify_token(token.token(), &state))
        .transpose()?
        .flatten();

    ensure_id!(payload, auth_id);

    match register_apns_nwc_impl(&state, payload).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("register_apns_nwc", e)),
    }
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

    let Some(apns_client) = state.apns_client.clone() else {
        return Ok(NwcWakeResponse::Rejected {
            code: "push_unavailable".to_string(),
            message: "APNS is not configured".to_string(),
            retry_after: None,
        });
    };

    let mut conn = state.db_pool.get()?;
    let registrations = ApnsNwcRegistration::find_by_nwc(
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

    let wallet_service_pubkey = hex::encode(payload.wallet_service_pubkey.serialize());
    for registration in registrations {
        apns_client
            .send_wake(
                &registration,
                &payload.relay,
                &payload.event_id,
                Some(&wallet_service_pubkey),
                None,
            )
            .await?;
    }

    Ok(NwcWakeResponse::Accepted)
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
