use crate::Storage;
use nostr_rs::Timestamp;
use nostr_sdk::{client::builder::ClientBuilder, Event, Kind, RelayPoolNotification};
use std::ops::Deref;

/// NIP-88 compliant oracle announcement listener.
///
/// This process listens to the connected relays for oracles making oracle announcements.
/// Listener is passed with a storage object which then stores the announcement in storage
/// to be used later for creating an offer contract.
///
/// The marketplace listener can be paired with `ddk::transport::NostDlc` to fetch announcements
/// and attestations from storage.
pub async fn marketplace_listener<S: Deref>(storage: &S, relays: Vec<&str>) -> anyhow::Result<()>
where
    S::Target: Storage,
{
    let client = ClientBuilder::new().build();
    for relay in relays {
        client.add_relay(relay).await?;
    }
    client.connect().await;
    let now = Timestamp::now();
    let oracle_filter = super::create_oracle_message_filter(now);

    client.subscribe(oracle_filter, None).await?;

    while let Ok(notification) = client.notifications().recv().await {
        match notification {
            RelayPoolNotification::Event {
                relay_url: _,
                subscription_id: _,
                event,
            } => handle_oracle_event(storage, *event),
            RelayPoolNotification::Shutdown => {
                tracing::error!("Relay disconnected.")
            }
            _ => (),
        }
    }

    Ok(())
}

fn handle_oracle_event<S: Deref>(storage: &S, event: Event)
where
    S::Target: Storage,
{
    match event.kind {
        Kind::Custom(89) => {
            tracing::info!("Oracle attestation. Saved to storage.")
        }
        Kind::Custom(88) => {
            let announcement = super::oracle_announcement_from_str(&event.content).unwrap();
            storage.save_announcement(announcement).unwrap();
            tracing::info!("Oracle announcement. Saved to storage.")
        }
        _ => (),
    }
}
