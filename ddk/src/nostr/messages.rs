//! # DLC Messages over Nostr
//!
//! This module implements Discreet Log Contract (DLC) message handling over the Nostr protocol,
//! following the draft NIP-88 specification for DLCs over Nostr.
//!
//! The implementation uses NIP-04 encryption for secure message transmission and handles
//! DLC protocol messages (kind 8888), oracle announcements (kind 88), and oracle attestations (kind 89).
//!
//! **NIP-88 Reference**: https://github.com/nostr-protocol/nips/pull/919
//!
//! Note: NIP-88 is still a draft specification and subject to change.

use crate::error::NostrError;
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

/// Creates a Nostr filter to listen for DLC protocol messages.
///
/// This filter targets events of kind 8888 (DLC_MESSAGE_KIND) sent to a specific public key
/// since a given timestamp. These events contain encrypted DLC protocol messages such as
/// offers, accepts, signs, and other contract-related communications.
///
/// # Arguments
/// * `since` - Timestamp to filter events from
/// * `public_key` - The recipient's public key to filter for
///
/// # Returns
/// A configured `Filter` for DLC message events
pub fn create_dlc_message_filter(since: Timestamp, public_key: PublicKey) -> Filter {
    Filter::new()
        .kind(DLC_MESSAGE_KIND)
        .since(since)
        .pubkey(public_key)
}

/// Creates a Nostr filter to listen for oracle announcements and attestations.
///
/// This filter targets events of kind 88 (oracle announcements) and kind 89 (oracle attestations)
/// since a given timestamp. These events are used by oracles to publish information about future
/// events they will attest to, and their subsequent attestations.
///
/// # Arguments
/// * `since` - Timestamp to filter events from
///
/// # Returns
/// A configured `Filter` for oracle message events
pub fn create_oracle_message_filter(since: Timestamp) -> Filter {
    Filter::new()
        .kinds([ORACLE_ANNOUNCMENT_KIND, ORACLE_ATTESTATION_KIND])
        .since(since)
}

/// Parses a DLC message from an encrypted Nostr event.
///
/// This function decrypts a Nostr event using NIP-04 encryption, decodes the base64 content,
/// and deserializes it into a DLC protocol message. The message content is expected to be
/// a serialized DLC message that was encrypted for the recipient.
///
/// # Arguments
/// * `event` - The Nostr event containing the encrypted DLC message
/// * `secret_key` - The recipient's secret key for decryption
///
/// # Returns
/// * `Ok(Message)` - Successfully parsed DLC message
/// * `Err(NostrError)` - If decryption, decoding, or parsing fails
///
/// # Errors
/// * `NostrError::MessageParsing` - If the message cannot be decoded or parsed
/// * `NostrError::Generic` - If there are issues with the underlying serialization
pub fn parse_dlc_msg_event(event: &Event, secret_key: &SecretKey) -> Result<Message, NostrError> {
    let decrypt = nip04::decrypt(secret_key, &event.pubkey, &event.content)?;

    let bytes = base64::decode(decrypt).map_err(|e| NostrError::MessageParsing(e.to_string()))?;

    let mut cursor = lightning::io::Cursor::new(bytes);

    let msg_type: u16 =
        Readable::read(&mut cursor).map_err(|e| NostrError::Generic(e.to_string()))?;

    let Some(wire) = read_dlc_message(msg_type, &mut cursor)
        .map_err(|e| NostrError::MessageParsing(e.to_string()))?
    else {
        return Err(NostrError::MessageParsing(
            "Couldn't read DLC message.".to_string(),
        ));
    };

    let message = match wire {
        WireMessage::Message(msg) => Ok(msg),
        // We could stll do segment chunks. Nostr relays can handle the large sizes,
        // but I'm running a custom relay so generic relays won't be able to handle.
        WireMessage::SegmentStart(_) | WireMessage::SegmentChunk(_) => {
            Err(NostrError::MessageParsing(
                "DLC message is not a valid message. Nostr should not be chunking messages."
                    .to_string(),
            ))
        }
    }?;

    tracing::info!(
        message = message_variant_name(&message),
        "Decrypted message from {}",
        event.pubkey.to_string()
    );

    Ok(message)
}

/// Handles a complete DLC message event and extracts all relevant information.
///
/// This is a higher-level function that validates the event kind, parses the DLC message,
/// and extracts the sender's public key. It ensures the event is actually a DLC message
/// event (kind 8888) before processing.
///
/// # Arguments
/// * `event` - The Nostr event to handle
/// * `secret_key` - The recipient's secret key for decryption
///
/// # Returns
/// * `Ok((SecpPublicKey, Message, Event))` - Tuple containing:
///   - The sender's secp256k1 public key (converted from Nostr pubkey)
///   - The parsed DLC message
///   - A clone of the original event
/// * `Err(NostrError)` - If the event is invalid or parsing fails
///
/// # Errors
/// * `NostrError::MessageParsing` - If the event is not kind 8888 or message parsing fails
pub fn handle_dlc_msg_event(
    event: &Event,
    secret_key: &SecretKey,
) -> Result<(SecpPublicKey, Message, Event), NostrError> {
    if event.kind != Kind::Custom(8_888) {
        return Err(NostrError::MessageParsing(
            "Event reveived was not DLC Message event (kind 8_888).".to_string(),
        ));
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

/// Creates and signs a Nostr event containing an encrypted DLC message.
///
/// This function serializes a DLC protocol message, encodes it as base64, encrypts it
/// using NIP-04 encryption for the specified recipient, and creates a signed Nostr event.
/// The resulting event can be published to Nostr relays for delivery.
///
/// # Arguments
/// * `to` - The recipient's Nostr public key
/// * `event_id` - Optional event ID to reply to (creates an 'e' tag)
/// * `msg` - The DLC message to send
/// * `keys` - The sender's Nostr keys for signing
///
/// # Returns
/// * `Ok(Event)` - The created and signed Nostr event ready for publishing
/// * `Err(NostrError)` - If encryption or event creation fails
///
/// # Event Structure
/// - **Kind**: 8888 (DLC_MESSAGE_KIND)
/// - **Content**: NIP-04 encrypted base64-encoded DLC message
/// - **Tags**:
///   - 'p' tag with recipient's public key
///   - 'e' tag with reply event ID (if provided)
pub fn create_dlc_msg_event(
    to: PublicKey,
    event_id: Option<EventId>,
    msg: Message,
    keys: &Keys,
) -> Result<Event, NostrError> {
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
