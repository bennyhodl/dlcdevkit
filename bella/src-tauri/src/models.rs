use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Pubkeys {
    // pub node_id: String,
    pub bitcoin: String,
}
