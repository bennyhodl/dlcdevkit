use crate::config::SeedConfig;
use crate::nostr::DLC_MESSAGE_KIND;
use crate::{io, RELAY_HOST};
use bitcoin::Network;
use dlc_messages::{message_handler::read_dlc_message, Message, WireMessage};
use lightning::{
    ln::wire::Type,
    util::ser::{Readable, Writeable},
};
use nostr_rs::{
    nips::nip04::{decrypt, encrypt},
    secp256k1::Secp256k1,
    Event, EventBuilder, EventId, Keys, Kind, PublicKey, SecretKey, Tag, Timestamp, Url,
};
use nostr_sdk::Client;

pub struct NostrDlcRelayHandler {
    pub keys: Keys,
    pub relay_url: Url,
    pub client: Client,
}

impl NostrDlcRelayHandler {
    pub fn new(
        seed_config: &SeedConfig,
        relay_host: &str,
        network: Network,
    ) -> anyhow::Result<NostrDlcRelayHandler> {
        let secp = Secp256k1::new();
        let seed = io::xprv_from_config(seed_config, network)?;
        // TODO: Seed to bytes is 78 not 64?
        let secret_key = SecretKey::from_slice(&seed.encode())?;
        let keys = Keys::new_with_ctx(&secp, secret_key.into());

        let relay_url = relay_host.parse()?;
        let client = Client::new(&keys);

        Ok(NostrDlcRelayHandler {
            keys,
            relay_url,
            client,
        })
    }

    pub fn public_key(&self) -> PublicKey {
        self.keys.public_key()
    }

    pub fn create_dlc_msg_event(
        &self,
        to: PublicKey,
        event_id: Option<EventId>,
        msg: Message,
    ) -> anyhow::Result<Event> {
        let mut bytes = msg.type_id().encode();
        bytes.extend(msg.encode());

        let content = encrypt(
            &self.keys.secret_key()?.clone(),
            &to,
            base64::encode(&bytes),
        )?;

        let p_tags = Tag::PublicKey {
            public_key: self.public_key(),
            relay_url: None,
            alias: None,
            uppercase: false,
        };

        let e_tags = event_id.map(|e| Tag::Event {
            event_id: e,
            relay_url: None,
            marker: None,
        });

        let tags = [Some(p_tags), e_tags]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        let event = EventBuilder::new(DLC_MESSAGE_KIND, content, tags).to_event(&self.keys)?;

        Ok(event)
    }

    pub fn parse_dlc_msg_event(&self, event: &Event) -> anyhow::Result<Message> {
        let decrypt = decrypt(
            &self.keys.secret_key().unwrap(),
            &event.pubkey,
            &event.content,
        )?;

        let bytes = base64::decode(decrypt)?;

        let mut cursor = lightning::io::Cursor::new(bytes);

        let msg_type: u16 = Readable::read(&mut cursor).unwrap();

        let Some(wire) = read_dlc_message(msg_type, &mut cursor).unwrap() else {
            return Err(anyhow::anyhow!("Couldn't read DLC message."));
        };

        match wire {
            WireMessage::Message(msg) => Ok(msg),
            WireMessage::SegmentStart(_) | WireMessage::SegmentChunk(_) => {
                Err(anyhow::anyhow!("Blah blah, something with a wire"))
            }
        }
    }

    pub fn handle_dlc_msg_event(&self, event: Event) {
        match event.kind {
            Kind::Custom(89) => tracing::info!("Oracle attestation kind."),
            Kind::Custom(88) => tracing::info!("Oracle announcement kind."),
            Kind::Custom(8_888) => tracing::info!("DLC message."),
            _ => tracing::info!("unknown"),
        }
    }

    pub async fn listen(&self) -> anyhow::Result<Client> {
        let client = Client::new(&self.keys);

        let since = Timestamp::now();

        client.add_relay(RELAY_HOST).await?;

        let msg_subscription =
            crate::nostr::util::create_dlc_message_filter(since, self.public_key());
        let oracle_subscription = crate::nostr::util::create_oracle_message_filter(since);

        client
            .subscribe(vec![msg_subscription, oracle_subscription], None)
            .await;

        client.connect().await;

        Ok(client)
    }
}
