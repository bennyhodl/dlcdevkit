use bitcoin::secp256k1::{PublicKey, SecretKey};
use nostr::Keys;

pub trait DeriveSigner {
    // Get the child key index for a given key_id.
    fn get_index_for_key_id(&self, key_id: [u8; 32]) -> u32;
    fn store_derived_key_id(
        &self,
        index: u32,
        key_id: [u8; 32],
        secret_key: SecretKey,
        public_key: PublicKey,
    );
    fn get_secret_key(&self, public_key: &PublicKey) -> SecretKey;
    fn import_address_to_storage(&self, address: &bitcoin::Address);
    // Get/store addresses for settlement from counterparty
}

pub struct SimpleDeriveSigner {}

impl DeriveSigner for SimpleDeriveSigner {
    fn get_index_for_key_id(&self, _key_id: [u8; 32]) -> u32 {
        1
    }

    fn store_derived_key_id(
        &self,
        _index: u32,
        _key_id: [u8; 32],
        _secret_key: SecretKey,
        _public_key: PublicKey,
    ) {
    }

    fn get_secret_key(&self, _public_key: &PublicKey) -> SecretKey {
        let keys = Keys::generate();
        let secret_key = keys.secret_key().unwrap();
        let bytes = secret_key.secret_bytes();
        bitcoin::secp256k1::SecretKey::from_slice(&bytes).expect("no bytes zone!")
    }

    fn import_address_to_storage(&self, _address: &bitcoin::Address) {
        ()
    }
}
