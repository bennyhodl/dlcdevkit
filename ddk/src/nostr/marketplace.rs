use std::ops::Deref;

use super::util;
use crate::Storage;
use nostr_rs::Timestamp;
use nostr_sdk::{client::builder::ClientBuilder, RelayPoolNotification};

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
    let oracle_filter = util::create_oracle_message_filter(now);

    client.subscribe(vec![oracle_filter], None).await?;

    while let Ok(notification) = client.notifications().recv().await {
        match notification {
            RelayPoolNotification::Event {
                relay_url: _,
                subscription_id: _,
                event,
            } => {
                util::handle_oracle_event(storage, *event);
            }
            RelayPoolNotification::Shutdown => {
                tracing::error!("Relay disconnected.")
            }
            _ => (),
        }
    }

    Ok(())
}
