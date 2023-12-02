use std::path::Path;
use crate::{io, ErnestDlcManager};
use dlc_messages::Message;
use nostr::{Keys, Filter, secp256k1::{SecretKey, Secp256k1}, Url};
use std::sync::{Arc, Mutex};
use serde::{Serialize, Deserialize};

pub struct NostrDlcHandler {
    pub keys: Keys,
    pub relay_url: Url,
    manager: Arc<Mutex<ErnestDlcManager>>,
}

impl NostrDlcHandler {
    pub fn new(wallet_name: String, relay_url: String, manager: Arc<Mutex<ErnestDlcManager>>) -> anyhow::Result<NostrDlcHandler> {
        let keys_file = io::get_ernest_dir().join(&wallet_name).join("nostr_keys");
        let keys = if Path::new(&keys_file).exists() {
            let secp = Secp256k1::new();
            let contents = std::fs::read_to_string(&keys_file)?;
            let secret_key = SecretKey::from_slice(contents.as_bytes())?;
            Keys::new_with_ctx(&secp, secret_key)
        } else {
            let keys = Keys::generate();
            let secret_key = &keys.secret_key()?;
            std::fs::write(keys_file, secret_key.secret_bytes())?;
            keys
        };

        let relay_url = relay_url.parse()?;

        Ok(NostrDlcHandler {
            keys,
            relay_url,
            manager
        })
    }
}

