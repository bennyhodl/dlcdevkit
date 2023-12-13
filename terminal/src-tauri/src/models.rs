use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Pubkeys {
    pub nostr: String,
    pub bitcoin: String,
}
