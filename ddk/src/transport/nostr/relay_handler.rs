use std::sync::Arc;

use crate::error::TransportError;
use crate::logger::Logger;
use crate::logger::{log_error, log_info, log_warn, WriteLog};
use crate::nostr::messages::{create_dlc_msg_event, handle_dlc_msg_event};
use crate::DlcDevKitDlcManager;
use crate::{nostr, Transport};
use crate::{Oracle, Storage};
use bitcoin::bip32::Xpriv;
use bitcoin::Network;
use nostr_rs::{secp256k1::Secp256k1, Keys, Timestamp, Url};
use nostr_sdk::{Client, RelayPoolNotification};
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub struct NostrDlc {
    pub keys: Keys,
    pub relay_url: Url,
    pub client: Client,
    pub logger: Arc<Logger>,
}

impl NostrDlc {
    #[tracing::instrument(skip(seed_bytes, logger))]
    pub async fn new(
        seed_bytes: &[u8; 64],
        relay_host: &str,
        network: Network,
        logger: Arc<Logger>,
    ) -> Result<NostrDlc, TransportError> {
        let secp = Secp256k1::new();
        let seed = Xpriv::new_master(network, seed_bytes)
            .map_err(|e| TransportError::Init(e.to_string()))?;
        let keys = Keys::new_with_ctx(&secp, seed.private_key.into());

        let relay_url = relay_host
            .parse()
            .map_err(|_| TransportError::Init("Could not parse relay url.".to_string()))?;
        let client = Client::new(keys.clone());
        client
            .add_relay(&relay_url)
            .await
            .map_err(|e| TransportError::Init(e.to_string()))?;
        client.connect().await;

        Ok(NostrDlc {
            keys,
            relay_url,
            client,
            logger,
        })
    }

    pub fn start<S: Storage, O: Oracle>(
        &self,
        mut stop_signal: watch::Receiver<bool>,
        manager: Arc<DlcDevKitDlcManager<S, O>>,
    ) -> JoinHandle<Result<(), TransportError>> {
        log_info!(
            self.logger,
            "Starting Nostr DLC listener. pubkey={} transport_public_key={}",
            self.keys.public_key().to_string(),
            self.public_key().to_string()
        );
        let nostr_client = self.client.clone();
        let keys = self.keys.clone();
        let logger = self.logger.clone();
        tokio::spawn(async move {
            let since = Timestamp::now();
            let msg_subscription =
                nostr::messages::create_dlc_message_filter(since, keys.public_key());
            nostr_client
                .subscribe(msg_subscription, None)
                .await
                .map_err(|e| TransportError::Listen(e.to_string()))?;
            log_info!(
                logger,
                "Listening for messages over nostr. pubkey={}",
                keys.public_key().to_string()
            );
            let mut notifications = nostr_client.notifications();
            loop {
                let logger_clone = logger.clone();
                tokio::select! {
                    _ = stop_signal.changed() => {
                        if *stop_signal.borrow() {
                            log_warn!(logger_clone, "Stopping nostr dlc message subscription.");
                            nostr_client.disconnect().await;
                            break;
                        }
                    },
                    Ok(notification) = notifications.recv() => {
                        if let RelayPoolNotification::Event {
                            relay_url: _,
                            subscription_id: _,
                            event,
                        } = notification {
                            let (pubkey, message, event) = match handle_dlc_msg_event(
                                &event,
                                keys.secret_key(),
                            ) {
                                Ok(msg) => {
                                    log_info!(logger_clone, "Received DLC nostr message. pubkey={}", msg.0.to_string());
                                    (msg.0, msg.1, msg.2)
                                },
                                Err(e) => {
                                    log_error!(logger_clone, "Could not parse event {}. error={}", event.id, e.to_string());
                                    continue;
                                }
                            };

                            match manager.on_dlc_message(&message, pubkey).await {
                                Ok(Some(msg)) => {
                                    let event = create_dlc_msg_event(
                                        event.pubkey,
                                        Some(event.id),
                                        msg,
                                        &keys,
                                    )
                                    .expect("no message");
                                    nostr_client
                                        .send_event(&event)
                                        .await
                                        .expect("Break out into functions.");
                                }
                                Ok(None) => (),
                                Err(_) => {
                                    // handle the error case and send
                                }
                            }
                        }
                    }
                }
            }
            Ok::<_, TransportError>(())
        })
    }
}
