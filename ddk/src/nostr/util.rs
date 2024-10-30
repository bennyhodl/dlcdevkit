use std::ops::Deref;

use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use lightning::io::Cursor;
use lightning::util::ser::Readable;
use nostr_rs::{Event, Filter, Kind, PublicKey as NostrPublicKey, Timestamp};

use crate::Storage;

use super::{DLC_MESSAGE_KIND, ORACLE_ANNOUNCMENT_KIND, ORACLE_ATTESTATION_KIND};

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

pub fn handle_oracle_event<S: Deref>(storage: &S, event: Event)
where
    S::Target: Storage,
{
    match event.kind {
        Kind::Custom(89) => {
            tracing::info!("Oracle attestation. Saved to storage.")
        }
        Kind::Custom(88) => {
            let announcement = oracle_announcement_from_str(&event.content).unwrap();
            storage.save_announcement(announcement).unwrap();
            tracing::info!("Oracle announcement. Saved to storage.")
        }
        _ => (),
    }
}

pub fn oracle_announcement_from_str(content: &str) -> anyhow::Result<OracleAnnouncement> {
    let bytes = base64::decode(content)?;
    let mut cursor = Cursor::new(bytes);
    Ok(OracleAnnouncement::read(&mut cursor)
        .map_err(|_| anyhow::anyhow!("could not get oracle announcement"))?)
}

pub fn oracle_attestation_from_str(content: &str) -> anyhow::Result<OracleAttestation> {
    let bytes = base64::decode(content)?;
    let mut cursor = Cursor::new(bytes);
    Ok(OracleAttestation::read(&mut cursor)
        .map_err(|_| anyhow::anyhow!("could not read oracle attestation"))?)
}
