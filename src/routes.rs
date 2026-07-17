use crate::auth::verify_token;
use crate::models::subscription_info::SubscriptionInfo;
use crate::{State, ALLOWED_LOCALHOST, ALLOWED_ORIGINS, ALLOWED_SUBDOMAIN};
use axum::headers::authorization::Bearer;
use axum::headers::{Authorization, Origin};
use axum::http::StatusCode;
use axum::{Extension, Json, TypedHeader};
use log::{error, info};
use serde::{Deserialize, Serialize};
use serde_json::json;
use web_push::{ContentEncoding, WebPushClient, WebPushMessageBuilder};
use web_push::{SubscriptionInfo as WebPushSubscriptionInfo, Urgency};

pub(crate) type HttpError = (StatusCode, String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub info: WebPushSubscriptionInfo,
    pub id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub title: String,
    pub body: String,
    pub icon: Option<String>,
}

pub(crate) fn ensure_request_id(
    payload_id: &mut Option<String>,
    auth_id: Option<String>,
) -> Result<(), HttpError> {
    match (payload_id.as_ref(), auth_id.as_ref()) {
        (None, None) => Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized: id required".to_string(),
        )),
        (None, Some(_)) => {
            *payload_id = auth_id;
            Ok(())
        }
        (Some(id), Some(auth_id)) if id != auth_id => Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized: id mismatch".to_string(),
        )),
        _ => Ok(()),
    }
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

pub async fn register(
    origin: Option<TypedHeader<Origin>>,
    auth: Option<TypedHeader<Authorization<Bearer>>>,
    Extension(state): Extension<State>,
    Json(mut payload): Json<RegisterRequest>,
) -> Result<Json<String>, HttpError> {
    if !state.self_hosted {
        validate_cors(origin)?;
    }

    let auth_id = auth
        .map(|TypedHeader(token)| verify_token(token.token(), &state))
        .transpose()?
        .flatten();
    ensure_request_id(&mut payload.id, auth_id)?;

    match register_impl(&state, payload).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("register", e)),
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

    let mut builder = WebPushMessageBuilder::new(&subscription_info);
    let content = json!(notification).to_string();
    builder.set_payload(ContentEncoding::Aes128Gcm, content.as_bytes());
    builder.set_vapid_signature(sig_builder);
    builder.set_urgency(Urgency::High);

    state.client.send(builder.build()?).await?;

    Ok(())
}

async fn broadcast_impl(state: &State, notification: Notification) -> anyhow::Result<()> {
    let mut conn = state.db_pool.get()?;
    let all = SubscriptionInfo::get_all(&mut conn)?;

    let mut futures = Vec::with_capacity(all.len());
    for item in all {
        let subscription_info = item.into_web_push();
        let fut = broadcast_individual(state, subscription_info, &notification);
        futures.push(fut);
    }
    futures::future::try_join_all(futures).await?;

    Ok(())
}

// todo add admin auth
pub async fn broadcast(
    Extension(state): Extension<State>,
    Json(notification): Json<Notification>,
) -> Result<Json<()>, HttpError> {
    match broadcast_impl(&state, notification).await {
        Ok(res) => Ok(Json(res)),
        Err(e) => Err(handle_anyhow_error("broadcast", e)),
    }
}

pub async fn health_check() -> Result<Json<()>, HttpError> {
    Ok(Json(()))
}

pub fn valid_origin(origin: &str) -> bool {
    ALLOWED_ORIGINS.contains(&origin)
        || origin.ends_with(ALLOWED_SUBDOMAIN)
        || origin.starts_with(ALLOWED_LOCALHOST)
}

pub(crate) fn validate_cors(origin: Option<TypedHeader<Origin>>) -> Result<(), HttpError> {
    if let Some(TypedHeader(origin)) = origin {
        if origin.is_null() {
            return Ok(());
        }

        let origin_str = origin.to_string();
        if valid_origin(&origin_str) {
            return Ok(());
        }

        return Err((StatusCode::NOT_FOUND, String::new()));
    }

    Ok(())
}

pub(crate) fn handle_anyhow_error(function: &str, err: anyhow::Error) -> HttpError {
    error!("Error in {function}: {err:?}");
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_id_uses_authenticated_identity_when_missing() {
        let mut id = None;
        ensure_request_id(&mut id, Some("authenticated".to_string())).unwrap();
        assert_eq!(id.as_deref(), Some("authenticated"));
    }

    #[test]
    fn request_id_rejects_identity_mismatch() {
        let mut id = Some("payload".to_string());
        assert_eq!(
            ensure_request_id(&mut id, Some("authenticated".to_string()))
                .unwrap_err()
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
}
