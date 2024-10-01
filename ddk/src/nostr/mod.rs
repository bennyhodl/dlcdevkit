use nostr_rs::Kind;

/// Nostr [dlc_messages::oracle_msgs::OracleAnnouncement] marketplace.
#[cfg(feature = "marketplace")]
pub mod marketplace;
/// Nostr related functions for DLC events.
pub mod util;

pub const DLC_MESSAGE_KIND: Kind = Kind::Custom(8_888);
pub const ORACLE_ANNOUNCMENT_KIND: Kind = Kind::Custom(88);
pub const ORACLE_ATTESTATION_KIND: Kind = Kind::Custom(89);
