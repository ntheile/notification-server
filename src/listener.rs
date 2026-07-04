use crate::apns::ApnsPushClient;
use crate::models::nwc_pubkey::NwcFilterInfo;
use crate::models::nwc_push_registration::NwcPushRegistration;
use crate::models::subscription_info::SubscriptionInfo;
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::PgConnection;
use log::{info, warn};
use nostr::{Event, Filter, Keys, Kind, Tag, Timestamp};
use nostr_sdk::{Client, RelayPoolNotification};
use serde_json::json;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::watch::Receiver;
use web_push::{
    ContentEncoding, IsahcWebPushClient, PartialVapidSignatureBuilder, Urgency, WebPushClient,
    WebPushMessageBuilder,
};

pub async fn start_listener(
    db_pool: Pool<ConnectionManager<PgConnection>>,
    mut receiver: Receiver<NwcFilterInfo>,
    sig_builder: PartialVapidSignatureBuilder,
    web_push_client: IsahcWebPushClient,
    apns_client: Option<ApnsPushClient>,
) -> anyhow::Result<()> {
    let keys = Keys::generate();
    loop {
        let client = Client::new(&keys);

        let filter: NwcFilterInfo = receiver.borrow().clone();
        let registered_client_count = filter.authors.len();
        let watched_wallet_count = filter.tagged.len();
        let configured_relay_count = filter.relays.len();
        let mut websocket_relay_count = 0usize;

        for relay in filter.relays.iter() {
            if relay.is_empty() {
                continue;
            }
            if relay.contains("localhost") {
                continue;
            }

            // todo may need to disable this
            let proxy = if relay.contains(".onion") {
                Some(SocketAddr::from_str("127.0.0.1:9050")?)
            } else {
                None
            };

            client.add_relay(relay.as_str(), proxy).await?;
            websocket_relay_count += 1;
        }
        client.connect().await;

        let nwc_requests = Filter::new()
            .kind(Kind::WalletConnectRequest)
            .pubkeys(filter.tagged)
            .since(Timestamp::now());

        client.subscribe(vec![nwc_requests]).await;

        println!(
            "Listening for NWC events: kind=23194 registered_clients={} wallet_pubkeys={} configured_relays={} websocket_relays={}",
            registered_client_count, watched_wallet_count, configured_relay_count, websocket_relay_count
        );

        let mut notifications = client.notifications();
        let mut matched_event_count = 0u64;
        loop {
            tokio::select! {
                Ok(notification) = notifications.recv() => {
                    match notification {
                        RelayPoolNotification::Event(url, event) => {
                            // check correct kind and has a p tag
                            if event.kind == Kind::WalletConnectRequest && event.tags.iter().any(|tag| matches!(tag, Tag::PubKey(_, _))) {
                                matched_event_count += 1;
                                println!(
                                    "NWC event matched: count={} relay={} event_id={}",
                                    matched_event_count,
                                    url,
                                    event.id.to_hex()
                                );
                                tokio::spawn({
                                    let sig_builder = sig_builder.clone();
                                    let db_pool = db_pool.clone();
                                    let web_push_client = web_push_client.clone();
                                    let apns_client = apns_client.clone();
                                    async move {
                                        let fut = handle_event(
                                            url.to_string(),
                                            event,
                                            db_pool,
                                            sig_builder,
                                            web_push_client,
                                            apns_client,
                                        );

                                        match tokio::time::timeout(Duration::from_secs(30), fut).await {
                                            Ok(Ok(_)) => {}
                                            Ok(Err(e)) => eprintln!("Error: {e}"),
                                            Err(_) => eprintln!("Timeout"),
                                        }
                                    }
                                });
                            }
                        }
                        RelayPoolNotification::Shutdown => {
                            println!("Relay pool shutdown");
                            break;
                        }
                        RelayPoolNotification::RelayStatus { .. } => {}
                        RelayPoolNotification::Stop => {}
                        RelayPoolNotification::Message(_, _) => {}
                    }
                }
                _ = receiver.changed() => {
                    println!(
                        "NWC watcher filter changed; reconnecting websocket relays after {} matched event(s)",
                        matched_event_count
                    );
                    break;
                }
            }
        }

        client.disconnect().await?;
    }
}

/// Handle a WalletConnectRequest event by sending a push notification.
async fn handle_event(
    relay: String,
    event: Event,
    db_pool: Pool<ConnectionManager<PgConnection>>,
    sig_builder: PartialVapidSignatureBuilder,
    web_push_client: IsahcWebPushClient,
    apns_client: Option<ApnsPushClient>,
) -> anyhow::Result<()> {
    let (apns_registrations, subscription) = {
        let mut conn = db_pool.get()?;
        let apns_registrations =
            NwcPushRegistration::find_apns_by_nwc_event(&mut conn, &event, &relay)?;
        let subscription = SubscriptionInfo::find_by_nwc_event(&mut conn, &event)?;
        (apns_registrations, subscription)
    };

    if let Some(apns_client) = apns_client {
        if apns_registrations.is_empty() {
            println!(
                "NWC event matched but no enabled APNS registration found: event_id={} client_pubkey={} wallet_service_pubkey={} relay={}",
                event.id.to_hex(),
                hex::encode(event.pubkey.serialize()),
                wallet_service_pubkey(&event).unwrap_or_else(|| "<missing>".to_string()),
                relay
            );
        }
        for registration in &apns_registrations {
            println!(
                "Sending APNS nwc_wake push for event {} to {}",
                event.id.to_hex(),
                registration.id
            );
            info!(
                "Sending APNS nwc_wake push for event {} to {}",
                event.id.to_hex(),
                registration.id
            );
            if let Err(err) = apns_client
                .send_wake_for_event(registration, &event, &relay)
                .await
            {
                warn!(
                    "Failed to send APNS nwc_wake push for event {} to {}: {}",
                    event.id.to_hex(),
                    registration.id,
                    err
                );
                if err.is_permanent() {
                    let mut conn = db_pool.get()?;
                    NwcPushRegistration::disable(&mut conn, registration)?;
                    info!(
                        "Disabled stale APNS NWC registration id={} author={} tagged={} relay={}",
                        registration.id,
                        registration.author,
                        registration.tagged,
                        registration.relay
                    );
                }
            }
        }
    } else if !apns_registrations.is_empty() {
        info!(
            "APNS is not configured; skipping {} APNS nwc_wake registrations",
            apns_registrations.len()
        );
    }

    // find the subscription info
    let Some((sub_info, name)) = subscription else {
        info!("No subscription found for event: {event:?}");
        return Ok(());
    };
    let subscription_info = sub_info.into_web_push();

    // build the signature
    let signature = sig_builder.add_sub_info(&subscription_info).build()?;

    // Now add payload and encrypt.
    let mut builder = WebPushMessageBuilder::new(&subscription_info);
    let content = json!({
        "title": format!("{name} has a pending payment!"),
        "body": "You have a pending payment. Open Mutiny to complete the transaction",
        "protocol": "nwc_wake",
        "version": "v1",
        "relay": relay,
        "event_id": event.id.to_hex(),
        "wallet_service_pubkey": event.tags.iter().find_map(|tag| {
            if let Tag::PubKey(pubkey, _) = tag {
                Some(hex::encode(pubkey.serialize()))
            } else {
                None
            }
        }),
    })
    .to_string();
    builder.set_payload(ContentEncoding::Aes128Gcm, content.as_bytes());
    builder.set_vapid_signature(signature);
    builder.set_urgency(Urgency::High); // todo maybe change this

    // Finally, send the notification!
    web_push_client.send(builder.build()?).await?;

    Ok(())
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
