use std::io::Cursor;

use dlc_sled_storage_provider::SledStorageProvider;
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use lightning::util::ser::Readable;
use nostr_sdk::{Event, Kind};

pub struct NostrDlcHandler {
    storage: SledStorageProvider,
}

impl NostrDlcHandler {
    pub fn new(storage: SledStorageProvider) -> Self {
        Self { storage }
    }

    pub fn receive_event(&self, event: Event) {
        match event.kind {
            Kind::Custom(88) => {
                let announcement = oracle_announcement_from_str(&event.content);
                println!("Oracle Announcement: {:?}", announcement);
            }
            Kind::Custom(89) => {
                let attestation = oracle_attestation_from_str(&event.content);
                println!("Oracle attestation: {:?}", attestation);
            }
            Kind::Custom(8_888) => println!("DLC message."),
            _ => println!("unknown {:?}", event),
        }
    }
}

fn oracle_announcement_from_str(content: &str) -> anyhow::Result<OracleAnnouncement> {
    let bytes = base64::decode(content)?;
    let mut cursor = Cursor::new(bytes);
    Ok(OracleAnnouncement::read(&mut cursor)
        .map_err(|_| anyhow::anyhow!("could not get oracle announcement"))?)
}

fn oracle_attestation_from_str(content: &str) -> anyhow::Result<OracleAttestation> {
    let bytes = base64::decode(content)?;
    let mut cursor = Cursor::new(bytes);
    Ok(OracleAttestation::read(&mut cursor)
        .map_err(|_| anyhow::anyhow!("could not read oracle attestation"))?)
}
