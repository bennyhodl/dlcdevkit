pub mod lightning;
pub mod nostr;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct PeerInformation {
    pub pubkey: String,
    pub host: String,
}
