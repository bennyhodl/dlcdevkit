use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use lightning::util::ser::Writeable;
use nostr::event::Error;
use nostr::{Event, EventBuilder, EventId, Keys, Kind, Tag};

/// Creates an Oracle Attestation event for nostr.
pub fn create_announcement_event(
    keys: &Keys,
    announcement: &OracleAnnouncement,
) -> Result<Event, Error> {
    let content = announcement.encode();
    let event = EventBuilder::new(Kind::Custom(88), base64::encode(content))
        .build(keys.public_key)
        .sign_with_keys(keys)?;
    Ok(event)
}

/// Creates an Oracle Attestation event for nostr.
pub fn create_attestation_event(
    keys: &Keys,
    attestation: &OracleAttestation,
    event_id: EventId,
) -> Result<Event, Error> {
    let content = attestation.encode();
    let event = EventBuilder::new(Kind::Custom(89), base64::encode(content))
        .tag(Tag::event(event_id))
        .build(keys.public_key)
        .sign_with_keys(keys)?;
    Ok(event)
}
