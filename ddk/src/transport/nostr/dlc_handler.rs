use crate::nostr::util::{oracle_announcement_from_str, oracle_attestation_from_str};
use nostr_sdk::{Event, Kind};

#[derive(Default)]
pub struct NostrDlcHandler;

impl NostrDlcHandler {
    pub fn receive_event(&self, event: Event) {
        match event.kind {
            Kind::Custom(88) => {
                let announcement = oracle_announcement_from_str(&event.content);
                tracing::info!("Oracle Announcement: {:?}", announcement);
            }
            Kind::Custom(89) => {
                let attestation = oracle_attestation_from_str(&event.content);
                tracing::info!("Oracle attestation: {:?}", attestation);
            }
            Kind::Custom(8_888) => tracing::info!("DLC message."),
            _ => tracing::info!("unknown {:?}", event),
        }
    }
}
