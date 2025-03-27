use bitcoin::key::Parity;
use bitcoin::secp256k1::PublicKey as BitcoinPublicKey;
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use lightning::io::Cursor;
use lightning::util::ser::Readable;
use nostr_rs::{Filter, Kind, PublicKey, Timestamp};

/// Nostr [dlc_messages::oracle_msgs::OracleAnnouncement] marketplace.
#[cfg(feature = "marketplace")]
pub mod marketplace;
pub mod messages;

pub const DLC_MESSAGE_KIND: Kind = Kind::Custom(8_888);
pub const ORACLE_ANNOUNCMENT_KIND: Kind = Kind::Custom(88);
pub const ORACLE_ATTESTATION_KIND: Kind = Kind::Custom(89);

pub fn bitcoin_to_nostr_pubkey(bitcoin_pk: &BitcoinPublicKey) -> PublicKey {
    // Convert to XOnlyPublicKey first
    let (xonly, _parity) = bitcoin_pk.x_only_public_key();

    // Create nostr public key from the x-only bytes
    PublicKey::from_slice(xonly.serialize().as_slice())
        .expect("Could not convert Bitcoin key to nostr key.")
}

pub fn nostr_to_bitcoin_pubkey(nostr_pk: &PublicKey) -> BitcoinPublicKey {
    let xonly = nostr_pk.xonly().expect("Could not get xonly public key.");
    BitcoinPublicKey::from_x_only_public_key(xonly, Parity::Even)
}

pub fn create_dlc_message_filter(since: Timestamp, public_key: PublicKey) -> Filter {
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_nostr_to_bitcoin_pubkey() {
        let nostr_pk = "7622b0ca2b5ad4d7441784a97bfc50c69d09853a640ad793a4fb9d238c7e0b15";
        let bitcoin_pk = "027622b0ca2b5ad4d7441784a97bfc50c69d09853a640ad793a4fb9d238c7e0b15";
        let nostr_pk_2 = bitcoin_to_nostr_pubkey(&BitcoinPublicKey::from_str(bitcoin_pk).unwrap());
        assert_eq!(nostr_pk_2.to_string(), nostr_pk);

        let nostr = PublicKey::from_str(nostr_pk).unwrap();
        let btc_pk = nostr_to_bitcoin_pubkey(&nostr);
        assert_eq!(btc_pk.to_string(), bitcoin_pk);
    }
}
