use std::ops::Deref;

use crate::Storage;
use nostr_rs::{Event, Filter, Kind, Timestamp};
use nostr_sdk::{client::builder::ClientBuilder, RelayPoolNotification};

use super::ORACLE_ANNOUNCMENT_KIND;

pub async fn marketplace_listener<S: Deref>(storage: &S, relays: Vec<&str>) -> anyhow::Result<()>
where
    S::Target: Storage,
{
    let client = ClientBuilder::new().build();
    client.add_relays(relays).await?;
    client.connect().await;
    let now = Timestamp::now();
    let oracle_filter = Filter::new().kinds([ORACLE_ANNOUNCMENT_KIND]).since(now);

    client.subscribe(vec![oracle_filter], None).await;
    loop {
        client
            .handle_notifications(|notification| async {
                match notification {
                    RelayPoolNotification::Event {
                        relay_url: _,
                        subscription_id: _,
                        event,
                    } => {
                        handle_oracle_msg(storage, *event);
                    }
                    RelayPoolNotification::Stop | RelayPoolNotification::Shutdown => {
                        tracing::error!("Relay disconnected.")
                    }
                    _ => (),
                }
                Ok(true)
            })
            .await
            .unwrap();
    }
}

pub fn handle_oracle_msg<S: Deref>(storage: &S, event: Event)
where
    S::Target: Storage,
{
    match event.kind {
        Kind::Custom(89) => {
            tracing::info!("Oracle attestation. Saved to storage.")
        }
        Kind::Custom(88) => {
            let announcement = crate::util::oracle_announcement_from_str(event.content()).unwrap();
            storage.save_announcement(announcement).unwrap();
            tracing::info!("Oracle announcement. Saved to storage.")
        }
        _ => (),
    }
}
