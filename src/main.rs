mod apns;
mod auth;
mod listener;
mod models;
mod routes;

use crate::apns::ApnsPushClient;
use crate::models::apns_nwc_registration::ApnsNwcRegistration;
use crate::models::nwc_pubkey::{NwcFilterInfo, NwcPubkeys};
use crate::models::MIGRATIONS;
use crate::routes::{
    broadcast, health_check, register, register_apns_nwc, register_nwc, valid_origin,
    validate_cors, wake_nwc,
};
use axum::headers::Origin;
use axum::http::{header, request::Parts, HeaderValue, StatusCode, Uri};
use axum::routing::{get, post};
use axum::{Extension, Router, TypedHeader};
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::PgConnection;
use diesel_migrations::MigrationHarness;
use secp256k1::{All, PublicKey, Secp256k1};
use std::sync::Arc;
use tokio::sync::{watch, Mutex};
use tower_http::cors::{AllowMethods, AllowOrigin, CorsLayer};
use web_push::{IsahcWebPushClient, PartialVapidSignatureBuilder, VapidSignatureBuilder};

const ALLOWED_ORIGINS: [&str; 6] = [
    "https://app.mutinywallet.com",
    "capacitor://localhost",
    "https://signet-app.mutinywallet.com",
    "http://localhost:3420",
    "http://localhost",
    "https://localhost",
];

const ALLOWED_SUBDOMAIN: &str = ".mutiny-web.pages.dev";
const ALLOWED_LOCALHOST: &str = "http://127.0.0.1:";

#[derive(Clone)]
pub struct State {
    pub db_pool: Pool<ConnectionManager<PgConnection>>,
    pub sig_builder: PartialVapidSignatureBuilder,
    pub auth_key: Option<PublicKey>,
    pub self_hosted: bool,
    pub client: IsahcWebPushClient,
    pub apns_client: Option<ApnsPushClient>,
    pub channel: Arc<Mutex<watch::Sender<NwcFilterInfo>>>,
    pub secp: Secp256k1<All>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file
    dotenv::dotenv().ok();
    pretty_env_logger::try_init()?;

    let self_hosted = std::env::var("SELF_HOST")
        .ok()
        .map(|s| s == "true" || s == "1")
        .unwrap_or(false);

    let key = std::env::var("VAPID_KEY").expect("VAPID_KEY must be set");
    let sig_builder = VapidSignatureBuilder::from_base64_no_sub(&key, web_push::URL_SAFE_NO_PAD)?;

    let auth_key = std::env::var("AUTH_KEY").ok();
    let auth_key = match auth_key {
        None => None,
        Some(data) => {
            let auth_key_bytes = hex::decode(data)?;
            Some(PublicKey::from_slice(&auth_key_bytes)?)
        }
    };

    // get values key from env
    let pg_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let port: u16 = std::env::var("NOTIFICATION_PORT")
        .ok()
        .map(|p| p.parse::<u16>())
        .transpose()?
        .unwrap_or(8080);

    // DB management
    let manager = ConnectionManager::<PgConnection>::new(&pg_url);
    let db_pool = Pool::builder()
        .max_size(10) // should be a multiple of 100, our database connection limit
        .test_on_check_out(true)
        .build(manager)
        .expect("Could not build connection pool");

    let mut connection = db_pool.get()?;
    connection
        .run_pending_migrations(MIGRATIONS)
        .expect("migrations could not run");

    let mut filter_info = NwcPubkeys::get_filter_info(&mut connection)?;
    filter_info.merge(ApnsNwcRegistration::get_filter_info(&mut connection)?);
    println!(
        "Initial NWC watcher filter: authors={} wallet_pubkeys={} relays={}",
        filter_info.authors.len(),
        filter_info.tagged.len(),
        filter_info.relays.len()
    );
    let (sender, receiver) = watch::channel(filter_info);
    let channel = Arc::new(Mutex::new(sender));

    drop(connection);

    let client = IsahcWebPushClient::new()?;
    let apns_client = ApnsPushClient::from_env()?;
    let secp = Secp256k1::gen_new();

    let state = State {
        db_pool: db_pool.clone(),
        sig_builder: sig_builder.clone(),
        auth_key,
        self_hosted,
        client: client.clone(),
        apns_client: apns_client.clone(),
        channel,
        secp,
    };

    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}")
        .parse()
        .expect("Failed to parse bind/port for webserver");

    // if the server is self hosted, allow all origins
    // otherwise, only allow the origins in ALLOWED_ORIGINS
    let cors_function = if self_hosted {
        |_: &HeaderValue, _request_parts: &Parts| true
    } else {
        |origin: &HeaderValue, _request_parts: &Parts| {
            let Ok(origin) = origin.to_str() else {
                return false;
            };

            valid_origin(origin)
        }
    };

    let server_router = Router::new()
        .route("/health-check", get(health_check))
        .route("/register", post(register))
        .route("/register-nwc", post(register_nwc))
        .route("/register-apns-nwc", post(register_apns_nwc))
        .route("/.well-known/nostr/nwc-wake", post(wake_nwc))
        .route("/broadcast", post(broadcast))
        .fallback(fallback)
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::predicate(cors_function))
                .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
                .allow_methods(AllowMethods::any()),
        )
        .layer(Extension(state));

    let server = axum::Server::bind(&addr).serve(server_router.into_make_service());

    println!("Webserver running on http://{addr}");

    // start the listener
    tokio::spawn(async move {
        loop {
            if let Err(e) = listener::start_listener(
                db_pool.clone(),
                receiver.clone(),
                sig_builder.clone(),
                client.clone(),
                apns_client.clone(),
            )
            .await
            {
                eprintln!("listener error: {e}")
            }
        }
    });

    let graceful = server.with_graceful_shutdown(async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to create Ctrl+C shutdown signal");
    });

    // Await the server to receive the shutdown signal
    if let Err(e) = graceful.await {
        eprintln!("shutdown error: {e}");
    }

    Ok(())
}

async fn fallback(origin: Option<TypedHeader<Origin>>, uri: Uri) -> (StatusCode, String) {
    if let Err((status, msg)) = validate_cors(origin) {
        return (status, msg);
    };

    (StatusCode::NOT_FOUND, format!("No route for {uri}"))
}
