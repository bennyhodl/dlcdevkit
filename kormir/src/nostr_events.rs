use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use ddk_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use ddk_messages::TlvRecord;
use nostr::event::Error;
use nostr::{Event, EventBuilder, EventId, Keys, Kind, Tag};

/// Creates an Oracle Announcement event for nostr.
///
/// The content is the announcement in its standalone TLV form, base64 encoded, which
/// is what other DLC clients on the relay expect to find there. Events published
/// before this carried the TLV body with no header; read them back with
/// [`TlvRecord::from_tlv_bytes_or_legacy`], since a relay's history cannot be
/// rewritten.
pub fn create_announcement_event(
    keys: &Keys,
    announcement: &OracleAnnouncement,
) -> Result<Event, Error> {
    let content = announcement.to_tlv_bytes();
    let event = EventBuilder::new(Kind::Custom(88), BASE64.encode(content))
        .build(keys.public_key)
        .sign_with_keys(keys)?;
    Ok(event)
}

/// Creates an Oracle Attestation event for nostr.
///
/// As with [`create_announcement_event`], the content is the standalone TLV form.
pub fn create_attestation_event(
    keys: &Keys,
    attestation: &OracleAttestation,
    event_id: EventId,
) -> Result<Event, Error> {
    let content = attestation.to_tlv_bytes();
    let event = EventBuilder::new(Kind::Custom(89), BASE64.encode(content))
        .tag(Tag::event(event_id))
        .build(keys.public_key)
        .sign_with_keys(keys)?;
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddk_messages::oracle_msgs::ANNOUNCEMENT_TYPE;

    const REAL_ANNOUNCEMENT_HEX: &str = "fdd824fd012a73740e61118e5d1c2c223c986b859a42c2cca56cb621d13ff8880ea856caf24aa29bc36a1e0d5c471b6a2baa68f98a413362c4e6a97f13c04b186395e9e0d8fcc3d07289c2ade25405c1c421b38c9322cd73fb2c89f42ce0730a35fae1f8875dfdd822c60001529fadc9958e1e1cf8ea29f05a25d67e04a0c21dcbfcb99b8b24d32f274795eb68e81101fdd8064e0004086e6f742d70616964067265706169641d6c6971756964617465642d62792d6d617475726174696f6e2d646174651d6c6971756964617465642d62792d70726963652d7468726573686f6c644d6c6f616e2d6d6174757265642d38313233313935633631653439376631323465623764336266626531323232613530326233306162343139363766306466323036306133656533366635623063";

    /// The relay carries the announcement in the same bytes the oracle would serve
    /// over HTTP, so a client reading the event needs no knowledge of kormir.
    #[test]
    fn announcement_event_content_is_the_standalone_tlv_record() {
        let announcement = OracleAnnouncement::from_tlv_hex(REAL_ANNOUNCEMENT_HEX).unwrap();
        let event = create_announcement_event(&Keys::generate(), &announcement).unwrap();

        let content = BASE64.decode(&event.content).unwrap();
        assert_eq!(content, announcement.to_tlv_bytes());
        assert_eq!(
            &content[..3],
            &[
                0xfd,
                (ANNOUNCEMENT_TYPE >> 8) as u8,
                ANNOUNCEMENT_TYPE as u8
            ]
        );
        assert_eq!(
            OracleAnnouncement::from_tlv_bytes(&content).unwrap(),
            announcement
        );
    }
}
