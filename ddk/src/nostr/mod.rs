use ddk_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use lightning::io::Cursor;
use lightning::util::ser::Readable;
use nostr_rs::{Filter, Kind, PublicKey as NostrPublicKey, Timestamp};

/// Nostr [ddk_messages::oracle_msgs::OracleAnnouncement] marketplace.
#[cfg(feature = "marketplace")]
pub mod marketplace;

pub const DLC_MESSAGE_KIND: Kind = Kind::Custom(8_888);
pub const ORACLE_ANNOUNCMENT_KIND: Kind = Kind::Custom(88);
pub const ORACLE_ATTESTATION_KIND: Kind = Kind::Custom(89);

pub fn create_dlc_message_filter(since: Timestamp, public_key: NostrPublicKey) -> Filter {
    Filter::new()
        .kind(DLC_MESSAGE_KIND)
        .since(since)
        .pubkey(public_key)
}

pub fn create_oracle_message_filter(since: Timestamp) -> Filter {
    Filter::new()
        .kinds([ORACLE_ANNOUNCMENT_KIND, ORACLE_ATTESTATION_KIND])
        .since(since)
}

pub fn oracle_announcement_from_str(content: &str) -> anyhow::Result<OracleAnnouncement> {
    let bytes = base64::decode(content)?;
    let mut cursor = Cursor::new(bytes);
    OracleAnnouncement::read(&mut cursor)
        .map_err(|_| anyhow::anyhow!("could not get oracle announcement"))
}

pub fn oracle_attestation_from_str(content: &str) -> anyhow::Result<OracleAttestation> {
    let bytes = base64::decode(content)?;
    let mut cursor = Cursor::new(bytes);
    OracleAttestation::read(&mut cursor)
        .map_err(|_| anyhow::anyhow!("could not read oracle attestation"))
}
