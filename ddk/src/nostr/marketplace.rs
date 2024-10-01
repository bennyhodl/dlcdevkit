use std::ops::Deref;

use crate::DdkStorage;
use nostr_rs::{Filter, Timestamp};
use nostr_sdk::{client::builder::ClientBuilder, RelayPoolNotification};

use super::ORACLE_ANNOUNCMENT_KIND;

pub async fn marketplace_listener<S: Deref>(storage: &S, relays: Vec<&str>) -> anyhow::Result<()>
where
    S::Target: DdkStorage,
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
                        crate::nostr::util::handle_dlc_msg_event(storage, *event);
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
