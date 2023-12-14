use std::{path::Path, time::Duration};
use crate::{io, ErnestDlcManager, RELAY_URL};
use dlc_messages::{Message, message_handler::read_dlc_message, WireMessage};
use dlc::secp256k1_zkp::PublicKey;
use lightning::{util::ser::{Writeable, Readable}, ln::wire::Type};
use nostr::{Keys, Kind, Filter, secp256k1::{Parity, PublicKey as NostrPublicKey, SecretKey, Secp256k1, XOnlyPublicKey}, Url, EventId, Event, nips::nip04::{encrypt, decrypt}, EventBuilder, Tag};
use nostr_sdk::{Client, RelayPoolNotification};
use std::sync::{Arc, Mutex};
use serde::Deserialize;

pub const DLC_MESSAGE_KIND: Kind = Kind::TextNote;

pub struct NostrDlcHandler {
    pub keys: Keys,
    pub relay_url: Url,
    manager: Arc<Mutex<ErnestDlcManager>>,
}

impl NostrDlcHandler {
    pub fn new(wallet_name: &str, relay_url: String, manager: Arc<Mutex<ErnestDlcManager>>) -> anyhow::Result<NostrDlcHandler> {
        let keys_file = io::get_ernest_dir().join(wallet_name).join("nostr_keys");
        let keys = if Path::new(&keys_file).exists() {
            let secp = Secp256k1::new();
            let contents = std::fs::read(&keys_file)?;
            let secret_key = SecretKey::from_slice(&contents)?;
            Keys::new_with_ctx(&secp, secret_key)
        } else {
            let keys = Keys::generate();
            let secret_key = keys.secret_key()?;
            std::fs::write(keys_file, secret_key.secret_bytes())?;
            keys
        };

        let relay_url = relay_url.parse()?;

        Ok(NostrDlcHandler {
            keys,
            relay_url,
            manager
        })
    }

    pub fn public_key(&self) -> XOnlyPublicKey {
        self.keys.public_key()
    }

    pub fn create_dlc_message_filter(&self) -> Filter {
        Filter::new().kind(DLC_MESSAGE_KIND)
    }

    pub fn create_dlc_msg_event(&self, to: XOnlyPublicKey, event_id: Option<EventId>, msg: Message) -> anyhow::Result<Event> {
        let mut bytes = msg.type_id().encode();
        bytes.extend(msg.encode());

        let content = encrypt(&self.keys.secret_key()?, &to, base64::encode(&bytes))?;

        let p_tags = Tag::PubKey(to, None);

        let e_tags = event_id.map(|e| Tag::Event(e, None, None));

        let tags = [Some(p_tags), e_tags]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();


        let event = EventBuilder::new(DLC_MESSAGE_KIND, content, &tags).to_event(&self.keys)?;

        Ok(event)
    }

    pub fn parse_dlc_msg_event(&self, event: &Event) -> anyhow::Result<Message> {
        let decrypt = decrypt(&self.keys.secret_key().unwrap(), &event.pubkey, &event.content)?;

        let bytes = base64::decode(decrypt)?;

        let mut cursor = lightning::io::Cursor::new(bytes);

        let msg_type: u16 = Readable::read(&mut cursor).unwrap();

        let Some(wire) = read_dlc_message(msg_type, &mut cursor).unwrap() else {
            return Err(anyhow::anyhow!("Couldn't read DLC message."))
        };
        
        match wire {
            WireMessage::Message(msg) => Ok(msg),
            WireMessage::SegmentStart(_) | WireMessage::SegmentChunk(_) => {
                Err(anyhow::anyhow!("Blah blah, something with a wire"))
            }
        }
    }

    pub fn handle_dlc_msg_event(&self, event: Event) -> anyhow::Result<Option<Event>> {
        if event.kind != DLC_MESSAGE_KIND {
            return Ok(None)
        };

        let msg = self.parse_dlc_msg_event(&event)?;

        let pubkey = PublicKey::from_slice(&event.pubkey.public_key(nostr::secp256k1::Parity::Even) .serialize())?;

        let mut dlc = self.manager.lock().unwrap();

        let msg_opts = dlc.on_dlc_message(&msg, pubkey)?;

        if let Some(msg) = msg_opts {
            let event = self.create_dlc_msg_event(event.pubkey, Some(event.id), msg)?;
            return Ok(Some(event))
        }

        Ok(None)
    }

    pub async fn listen(&self) -> anyhow::Result<Client> {
        let client = Client::new(&self.keys);

        client.add_relay(RELAY_URL, None).await?;

        let subscription = self.create_dlc_message_filter();

        client.subscribe(vec![subscription]).await;

        client.connect().await;

        Ok(client)
    }
}

pub fn handle_relay_event(event: RelayPoolNotification) {
    match event {
        RelayPoolNotification::Event(_, e) => {
            println!("Received event: {}", e.content);
        },
        _ => ()
    }
}

