use crate::nostr::nostr_to_bitcoin_pubkey;
use crate::nostr::{DLC_MESSAGE_KIND, ORACLE_ANNOUNCMENT_KIND, ORACLE_ATTESTATION_KIND};
use crate::util::ser::message_variant_name;
use dlc::secp256k1_zkp::PublicKey as SecpPublicKey;
use dlc_messages::message_handler::read_dlc_message;
use dlc_messages::{Message, WireMessage};
use lightning::ln::wire::Type;
use lightning::util::ser::{Readable, Writeable};
use nostr_rs::nips::nip04;
use nostr_rs::{
    Event, EventBuilder, EventId, Filter, Keys, Kind, PublicKey, SecretKey, Tag, Timestamp,
};

/// Listens for DLC messages with event kind 8,888. These are messages such as
/// these are any of the [dlc_messages::Message]
pub fn create_dlc_message_filter(since: Timestamp, public_key: PublicKey) -> Filter {
    Filter::new()
        .kind(DLC_MESSAGE_KIND)
        .since(since)
        .pubkey(public_key)
}

/// Listens for oracle attestations and announcements. Kind 89 and 88.
pub fn create_oracle_message_filter(since: Timestamp) -> Filter {
    Filter::new()
        .kinds([ORACLE_ANNOUNCMENT_KIND, ORACLE_ATTESTATION_KIND])
        .since(since)
}

pub fn parse_dlc_msg_event(event: &Event, secret_key: &SecretKey) -> anyhow::Result<Message> {
    let decrypt = nip04::decrypt(secret_key, &event.pubkey, &event.content)?;

    let bytes = base64::decode(decrypt)?;

    let mut cursor = lightning::io::Cursor::new(bytes);

    let msg_type: u16 = Readable::read(&mut cursor).unwrap();

    let Some(wire) = read_dlc_message(msg_type, &mut cursor).unwrap() else {
        return Err(anyhow::anyhow!("Couldn't read DLC message."));
    };

    let message = match wire {
        WireMessage::Message(msg) => Ok(msg),
        WireMessage::SegmentStart(_) | WireMessage::SegmentChunk(_) => {
            Err(anyhow::anyhow!("Blah blah, something with a wire"))
        }
    }?;

    tracing::info!(
        message = message_variant_name(&message),
        "Decrypted message from {}",
        event.pubkey.to_string()
    );

    Ok(message)
}

pub fn handle_dlc_msg_event(
    event: &Event,
    secret_key: &SecretKey,
) -> anyhow::Result<(SecpPublicKey, Message, Event)> {
    if event.kind != Kind::Custom(8_888) {
        return Err(anyhow::anyhow!("Event reveived was not DLC Message event."));
    }
    tracing::info!(
        kind = 8_888,
        pubkey = event.pubkey.to_string(),
        "Received DLC message event."
    );

    let message = parse_dlc_msg_event(event, secret_key)?;

    let pubkey = nostr_to_bitcoin_pubkey(&event.pubkey);

    Ok((pubkey, message, event.clone()))
}

pub fn create_dlc_msg_event(
    to: PublicKey,
    event_id: Option<EventId>,
    msg: Message,
    keys: &Keys,
) -> anyhow::Result<Event> {
    let mut bytes = msg.type_id().encode();
    bytes.extend(msg.encode());

    let content = nip04::encrypt(&keys.secret_key().clone(), &to, base64::encode(&bytes))?;

    let p_tags = Tag::public_key(to);

    let e_tags = event_id.map(Tag::event);

    let tags = [Some(p_tags), e_tags]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    let event = EventBuilder::new(DLC_MESSAGE_KIND, content)
        .tags(tags)
        .sign_with_keys(keys)?;

    Ok(event)
}
