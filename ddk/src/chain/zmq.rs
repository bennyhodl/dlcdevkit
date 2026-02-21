use crate::error::Error;
use crate::logger::{log_error, log_info, log_warn};
use bitcoin::hashes::sha256d::Hash;
use bitcoin::BlockHash;
use lightning::log_debug;
use lightning::util::logger::Logger;
use std::sync::Arc;
use tokio::select;
use tokio::sync::watch;
use tokio::sync::watch::error::SendError;
use zeromq::prelude::*;
use zeromq::SubSocket;

const HASH_BLOCK_TOPIC: &str = "hashblock";

#[derive(Debug, Clone, PartialEq)]
pub enum ZeromqMessage {
    NewBlock(BlockHash),
}

impl ZeromqMessage {
    fn init_dummy() -> Self {
        Self::new_blockhash([0u8; 32])
    }

    fn new_blockhash(data: [u8; 32]) -> Self {
        let hash = Hash::from_bytes_ref(&data);
        let blockhash = BlockHash::from_raw_hash(*hash);
        ZeromqMessage::NewBlock(blockhash)
    }
}

impl std::fmt::Display for ZeromqMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NewBlock(blockhash) => write!(f, "New block found: {blockhash}"),
        }
    }
}

pub struct ZeromqClient {
    logger: Arc<dyn Logger + Send + Sync>,
    sender: watch::Sender<ZeromqMessage>,
}

impl ZeromqClient {
    pub async fn new(
        endpoint: &str,
        logger: Arc<dyn Logger + Send + Sync>,
        stop: watch::Receiver<bool>,
    ) -> Result<Self, Error> {
        let mut socket = SubSocket::new();
        socket.connect(endpoint).await?;

        // Any subscribers created will consider the first value read. Essentially it will be
        // ignored. So putting a dummy message in here prevents us from needing to know what
        // the current block height is on startup.
        let (sender, _) = watch::channel(ZeromqMessage::init_dummy());

        let sender_clone = sender.clone();
        let logger_clone = logger.clone();
        tokio::spawn(
            async move { listen_and_notify(socket, sender_clone, logger_clone, stop).await },
        );

        Ok(Self { logger, sender })
    }

    pub fn subscribe(&self) -> watch::Receiver<ZeromqMessage> {
        log_debug!(self.logger, "Adding ZMQ notification subscriber");
        self.sender.subscribe()
    }
}

impl std::fmt::Debug for ZeromqClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZeromqClient")
            // The other fields don't implement debug
            .field("sender", &self.sender)
            .finish()
    }
}

async fn listen_and_notify(
    mut socket: SubSocket,
    sender: watch::Sender<ZeromqMessage>,
    logger: Arc<dyn Logger + Send + Sync>,
    mut stop: watch::Receiver<bool>,
) {
    log_info!(logger, "Starting ZMQ subscriber loop");
    if let Err(err) = socket.subscribe(HASH_BLOCK_TOPIC).await {
        log_error!(
            logger,
            "Error subscribing to the ZMQ {} topic: {}",
            HASH_BLOCK_TOPIC,
            err
        );
        return;
    }

    loop {
        select! {
            _ = stop.changed() => {
                log_info!(logger, "ZMQ client received stop signal. Exiting.");
                break;
            }
            message = socket.recv() => {
                let message = match message {
                    Ok(message) => message,
                    Err(err) => {
                        log_error!(logger, "Error received from ZMQ: {}", err);
                        continue;
                    }
                };
                log_debug!(logger, "ZMQ message received: {:?}", message);

                let Some(body) = message.get(1) else {
                    log_error!(logger, "Message from ZMQ did not contain a body: {:?}", message);
                    continue;
                };
                if body.len() != 32 {
                    log_warn!(logger, "Message from ZMQ was not 32-bytes: {}", body.len());
                    continue;
                }
                // From https://github.com/bitcoin/bitcoin/blob/master/doc/zmq.md#hashblock
                // The body is a "32-byte block hash in reversed byte order."
                let mut hash = [0u8; 32];
                hash.copy_from_slice(body);

                let blockhash = hex::encode(hash);
                match handle_message_body(hash, &sender) {
                    Ok(_) => log_debug!(logger, "Blockhash {} successfully sent from ZMQ client", blockhash),
                    Err(err) => log_warn!(logger, "New block notification failed due to no active receivers: {}", err)
                };
            }
        }
    }
}

fn handle_message_body(
    mut body: [u8; 32],
    sender: &watch::Sender<ZeromqMessage>,
) -> Result<(), SendError<ZeromqMessage>> {
    body.reverse();
    let message = ZeromqMessage::new_blockhash(body);

    sender.send(message.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_message_body_blockhash() {
        let body: [u8; 32] =
            hex::decode("0000000000000000000152c5a30d731fdce51fe8c07d5cf227015b386188e5a2")
                .unwrap()
                .try_into()
                .unwrap();
        let (sender, receiver) = watch::channel(ZeromqMessage::init_dummy());

        handle_message_body(body, &sender).unwrap();

        let body: [u8; 32] =
            hex::decode("a2e58861385b0127f25c7dc0e81fe5dc1f730da3c55201000000000000000000")
                .unwrap()
                .try_into()
                .unwrap();
        let expected = ZeromqMessage::new_blockhash(body);

        assert!(receiver.has_changed().unwrap());
        assert_eq!(expected, *receiver.borrow());
    }
}
