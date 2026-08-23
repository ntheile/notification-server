use crate::apns::ApnsPushClient;
use crate::models::nwc_invoice_monitor::{NwcInvoiceMonitor, SettlementDelivery};
use crate::models::nwc_push_registration::NwcPushRegistration;
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::PgConnection;
use log::{info, warn};
use std::time::Duration;

const CLAIM_BATCH_SIZE: i64 = 20;

pub async fn run(
    pool: Pool<ConnectionManager<PgConnection>>,
    apns: ApnsPushClient,
) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        interval.tick().await;
        let due = {
            let mut conn = pool.get()?;
            NwcInvoiceMonitor::disable_expired(&mut conn)?;
            NwcInvoiceMonitor::disable_exhausted(&mut conn)?;
            NwcInvoiceMonitor::claim_due(&mut conn, CLAIM_BATCH_SIZE)?
        };
        for claimed in due {
            let monitor = claimed.monitor;
            let registration = {
                let mut conn = pool.get()?;
                NwcPushRegistration::find_apns_for_monitor(
                    &mut conn,
                    &monitor.id,
                    &monitor.client_pubkey,
                    &monitor.wallet_service_pubkey,
                    &monitor.relay,
                )?
            };
            let Some(registration) = registration else {
                let mut conn = pool.get()?;
                NwcInvoiceMonitor::disable(
                    &mut conn,
                    &monitor.id,
                    &monitor.request_event_id,
                    &monitor.wallet_service_pubkey,
                    &monitor.relay,
                )?;
                continue;
            };
            let result = match claimed.delivery {
                SettlementDelivery::Background => {
                    apns.send_settlement_background(
                        &registration,
                        &monitor.relay,
                        &monitor.request_event_id,
                        &monitor.wallet_service_pubkey,
                    )
                    .await
                }
                SettlementDelivery::AlertFallback => {
                    apns.send_settlement_alert(
                        &registration,
                        &monitor.relay,
                        &monitor.request_event_id,
                        &monitor.wallet_service_pubkey,
                    )
                    .await
                }
            };
            match result {
                Ok(receipt) => {
                    let mut conn = pool.get()?;
                    NwcInvoiceMonitor::mark_delivered(&mut conn, &monitor, claimed.delivery)?;
                    info!(
                        "NWC settlement wake accepted event_id={} registration_id={} delivery={:?} payload_bytes={}",
                        monitor.request_event_id,
                        monitor.id,
                        claimed.delivery,
                        receipt.payload_bytes
                    );
                }
                Err(error) => {
                    warn!(
                        "NWC settlement wake failed event_id={} registration_id={} delivery={:?} error={}",
                        monitor.request_event_id, monitor.id, claimed.delivery, error
                    );
                    if error.is_permanent() {
                        let mut conn = pool.get()?;
                        NwcPushRegistration::disable(&mut conn, &registration)?;
                        NwcInvoiceMonitor::disable(
                            &mut conn,
                            &monitor.id,
                            &monitor.request_event_id,
                            &monitor.wallet_service_pubkey,
                            &monitor.relay,
                        )?;
                    } else {
                        let mut conn = pool.get()?;
                        NwcInvoiceMonitor::retry_delivery(&mut conn, &monitor, claimed.delivery)?;
                    }
                }
            }
        }
    }
}
