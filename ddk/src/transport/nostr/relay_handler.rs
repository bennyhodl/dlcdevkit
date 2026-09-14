use std::sync::Arc;

use crate::error::TransportError;
use crate::logger::Logger;
use crate::logger::{log_error, log_info, log_warn, WriteLog};
use crate::nostr::messages::{create_dlc_msg_event, handle_dlc_msg_event};
use crate::DlcDevKitDlcManager;
use crate::{nostr, Transport};
use crate::{Oracle, Storage};
use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::secp256k1::Secp256k1;
use bitcoin::Network;
use nostr_rs::key::{Keys, SecretKey};
use nostr_rs::types::{Timestamp, Url};
use nostr_sdk::client::{Client, ClientNotification};
use nostr_sdk::prelude::StreamExt;
use std::str::FromStr;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// The NIP-06 derivation path of the nostr identity: the first account of
/// nostr's registered coin type, hardened up to the account level.
///
/// The wallet lives under `m/84'/…` and the contract keys under `m/420'/…`,
/// so a key that leaks through the nostr stack reveals nothing about either.
pub const NOSTR_KEY_PATH: &str = "m/44'/1237'/0'/0/0";

/// How the nostr transport derives its identity key from the wallet seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NostrKeyDerivation {
    /// The NIP-06 child at [`NOSTR_KEY_PATH`]. This is what
    /// [`NostrDlc::new`] uses.
    Nip06,
    /// The raw BIP32 master private key of the wallet seed.
    ///
    /// This was the identity before 2.0.0-rc.4. It ties the wallet, every
    /// contract key, and the nostr identity to one secret: whatever the nostr
    /// stack does with its key, it does with the wallet master key. Keep it
    /// only for a deployment whose peers already know the old identity, and
    /// plan the move to [`NostrKeyDerivation::Nip06`].
    #[deprecated(
        since = "2.0.0-rc.4",
        note = "the nostr identity is the wallet master private key; use NostrKeyDerivation::Nip06"
    )]
    Master,
}

/// Derives the nostr identity keys from the wallet seed with `derivation`.
pub fn nostr_keys(
    seed_bytes: &[u8; 64],
    network: Network,
    derivation: NostrKeyDerivation,
) -> Result<Keys, TransportError> {
    let secp = Secp256k1::new();
    let master =
        Xpriv::new_master(network, seed_bytes).map_err(|e| TransportError::Init(e.to_string()))?;
    let secret_key = match derivation {
        NostrKeyDerivation::Nip06 => {
            let path = DerivationPath::from_str(NOSTR_KEY_PATH)
                .map_err(|e| TransportError::Init(e.to_string()))?;
            master
                .derive_priv(&secp, &path)
                .map_err(|e| TransportError::Init(e.to_string()))?
                .private_key
        }
        #[allow(deprecated)]
        NostrKeyDerivation::Master => master.private_key,
    };
    let secret_key = SecretKey::from_slice(&secret_key.secret_bytes())
        .map_err(|e| TransportError::Init(e.to_string()))?;
    Ok(Keys::new(secret_key))
}

pub struct NostrDlc {
    pub keys: Keys,
    pub relay_url: Url,
    pub client: Client,
    pub logger: Arc<Logger>,
}

impl NostrDlc {
    /// Creates the transport with a NIP-06 identity derived from the wallet
    /// seed ([`NostrKeyDerivation::Nip06`]).
    ///
    /// Before 2.0.0-rc.4 the identity was the wallet's master private key. A
    /// node upgraded from such a release gets a new nostr public key; use
    /// [`NostrDlc::new_with_derivation`] with
    /// [`NostrKeyDerivation::Master`] to keep the old one.
    #[tracing::instrument(skip(seed_bytes, logger))]
    pub async fn new(
        seed_bytes: &[u8; 64],
        relay_host: &str,
        network: Network,
        logger: Arc<Logger>,
    ) -> Result<NostrDlc, TransportError> {
        Self::new_with_derivation(
            seed_bytes,
            relay_host,
            network,
            NostrKeyDerivation::Nip06,
            logger,
        )
        .await
    }

    /// Creates the transport with the identity `derivation` selects.
    #[tracing::instrument(skip(seed_bytes, logger))]
    pub async fn new_with_derivation(
        seed_bytes: &[u8; 64],
        relay_host: &str,
        network: Network,
        derivation: NostrKeyDerivation,
        logger: Arc<Logger>,
    ) -> Result<NostrDlc, TransportError> {
        let keys = nostr_keys(seed_bytes, network, derivation)?;

        let relay_url: Url = relay_host
            .parse()
            .map_err(|_| TransportError::Init("Could not parse relay url.".to_string()))?;
        let client = Client::new();
        client
            .add_relay(relay_url.as_str())
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
                .subscribe(msg_subscription)
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
                    Some(notification) = notifications.next() => {
                        if let ClientNotification::Event {
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

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::bip32::ChildNumber;

    const SEED: [u8; 64] = [7u8; 64];

    #[test]
    fn nip06_identity_is_a_hardened_child_of_the_seed() {
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Regtest, &SEED).unwrap();
        let expected = master
            .derive_priv(
                &secp,
                &[
                    ChildNumber::from_hardened_idx(44).unwrap(),
                    ChildNumber::from_hardened_idx(1237).unwrap(),
                    ChildNumber::from_hardened_idx(0).unwrap(),
                    ChildNumber::from_normal_idx(0).unwrap(),
                    ChildNumber::from_normal_idx(0).unwrap(),
                ],
            )
            .unwrap()
            .private_key;

        let keys = nostr_keys(&SEED, Network::Regtest, NostrKeyDerivation::Nip06).unwrap();
        assert_eq!(keys.secret_key().as_secret_bytes(), &expected[..]);
    }

    #[test]
    fn nip06_identity_differs_from_the_wallet_master_key() {
        let master = Xpriv::new_master(Network::Regtest, &SEED).unwrap();
        let keys = nostr_keys(&SEED, Network::Regtest, NostrKeyDerivation::Nip06).unwrap();
        assert_ne!(keys.secret_key().as_secret_bytes(), &master.private_key[..]);

        #[allow(deprecated)]
        let legacy = nostr_keys(&SEED, Network::Regtest, NostrKeyDerivation::Master).unwrap();
        assert_eq!(
            legacy.secret_key().as_secret_bytes(),
            &master.private_key[..]
        );
    }
}
