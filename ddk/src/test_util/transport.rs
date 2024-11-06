use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::{ddk::DlcDevKitDlcManager, Oracle, Storage, Transport};
use bitcoin::{key::{self, Keypair}, secp256k1::{All, PublicKey, Secp256k1}};
use crossbeam::channel::{unbounded, Receiver, Sender};
use dlc_messages::Message;

pub struct MemoryTransport {
    pub receiver: Receiver<(Message, PublicKey)>,
    pub sender: Sender<(Message, PublicKey)>,
    pub counterparty_transport: HashMap<PublicKey, Sender<(Message, PublicKey)>>,
    pub keypair: Keypair
}

impl MemoryTransport {
    pub fn new(secp: &Secp256k1<All>) -> Self {
        let (sender, receiver) = unbounded();
        let keypair = Keypair::new(secp, &mut key::rand::thread_rng());
        Self {
            receiver,
            sender,
            counterparty_transport: HashMap::new(),
            keypair,
        }
    }

    pub fn add_counterparty(&mut self, counterparty: PublicKey, sender: Sender<(Message, PublicKey)>) {
        self.counterparty_transport.insert(counterparty, sender);
    }
}

#[async_trait::async_trait]
impl Transport for MemoryTransport {
    fn name(&self) -> String {
        "memory transport".to_string()
    }

    fn public_key(&self) -> PublicKey {
        self.keypair.public_key()
    }

    fn send_message(&self, counterparty: PublicKey, message: Message) {
        let connected_counterparty = self.counterparty_transport.get(&counterparty);
        if let Some(counterparty) = connected_counterparty { 
            counterparty
            .send((message, self.keypair.public_key()))
            .expect("could not send message to counterparty")
        } else {
            tracing::error!("No counterparty connected.")
        }
    }

    async fn listen(&self) {
        tracing::info!("Listening on memory listener")
    }

    async fn receive_messages<S: Storage, O: Oracle>(
        &self,
        manager: Arc<DlcDevKitDlcManager<S, O>>,
    ) {
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        loop {
            timer.tick().await;
            if let Ok(msg) = self.receiver.recv() {
                match manager.on_dlc_message(&msg.0, msg.1) {
                    Ok(s) => {
                        if let Some(reply) = s {
                            self.send_message(msg.1, reply);
                        } else {
                            tracing::info!("Handled on_dlc_message.");
                        }

                    }
                    Err(e) => tracing::error!(error=e.to_string(), "In memory transport error on dlc message.")
                }
            }
            
        }
    }

    async fn connect_outbound(&self, _pubkey: PublicKey, _host: &str) {
        unreachable!("no need to connect to counterparty")
    }

}
