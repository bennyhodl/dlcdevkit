use std::sync::Arc;

use crate::DlcDevKitDlcManager;
use crate::{Oracle, Storage};
use bitcoin::bip32::Xpriv;
use bitcoin::Network;
use nostr_rs::{secp256k1::Secp256k1, Keys, PublicKey, SecretKey, Timestamp, Url};
use nostr_sdk::{Client, RelayPoolNotification};

pub struct NostrDlc {
    pub keys: Keys,
    pub relay_url: Url,
    pub client: Client,
}

impl NostrDlc {
    pub fn new(
        seed_bytes: &[u8; 32],
        relay_host: &str,
        network: Network,
    ) -> anyhow::Result<NostrDlc> {
        let secp = Secp256k1::new();
        let seed = Xpriv::new_master(network, seed_bytes)?;
        let secret_key = SecretKey::from_slice(&seed.encode())?;
        let keys = Keys::new_with_ctx(&secp, secret_key.into());

        let relay_url = relay_host.parse()?;
        let client = Client::new(&keys);

        Ok(NostrDlc {
            keys,
            relay_url,
            client,
        })
    }

    pub fn public_key(&self) -> PublicKey {
        self.keys.public_key()
    }

    pub async fn listen(&self) -> anyhow::Result<Client> {
        let client = Client::new(&self.keys);

        let since = Timestamp::now();

        client.add_relay(&self.relay_url).await?;

        let msg_subscription = super::messages::create_dlc_message_filter(since, self.public_key());
        // Removing the oracle messages for right now.
        // let oracle_subscription = super::messages::create_oracle_message_filter(since);

        client.subscribe(vec![msg_subscription], None).await?;

        client.connect().await;

        Ok(client)
    }

    pub async fn receive_dlc_messages<S: Storage, O: Oracle>(
        &self,
        manager: Arc<DlcDevKitDlcManager<S, O>>,
    ) {
        while let Ok(notification) = self.client.notifications().recv().await {
            match notification {
                RelayPoolNotification::Event {
                    relay_url: _,
                    subscription_id: _,
                    event,
                } => {
                    let (pubkey, message, event) = match super::messages::handle_dlc_msg_event(
                        &event,
                        &self.keys.secret_key(),
                    ) {
                        Ok(msg) => (msg.0, msg.1, msg.2),
                        Err(_) => {
                            tracing::error!("Could not parse event {}", event.id);
                            continue;
                        }
                    };

                    match manager.on_dlc_message(&message, pubkey) {
                        Ok(Some(msg)) => {
                            let event = super::messages::create_dlc_msg_event(
                                event.pubkey,
                                Some(event.id),
                                msg,
                                &self.keys,
                            )
                            .expect("no message");
                            self.client
                                .send_event(event)
                                .await
                                .expect("Break out into functions.");
                        }
                        Ok(None) => (),
                        Err(_) => {
                            // handle the error case and send
                        }
                    }
                }
                other => println!("Other event: {:?}", other),
            }
        }
    }
}
