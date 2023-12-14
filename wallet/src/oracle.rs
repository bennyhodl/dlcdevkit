use bitcoin::{secp256k1::Secp256k1, KeyPair, XOnlyPublicKey};

#[derive(Default, Debug)]
pub struct Oracle;

impl dlc_manager::Oracle for Oracle {
    fn get_public_key(&self) -> bitcoin::XOnlyPublicKey {
        let secp = Secp256k1::new();
        let keypair = KeyPair::new(&secp, &mut rand::thread_rng());
        XOnlyPublicKey::from_keypair(&keypair).0
    }

    fn get_attestation(
        &self,
        _event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAttestation, dlc_manager::error::Error> {
        unimplemented!("attestation")
    }

    fn get_announcement(
        &self,
        _event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAnnouncement, dlc_manager::error::Error> {
        unimplemented!("announcement")
    }
}
