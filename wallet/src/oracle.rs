#[derive(Default, Debug)]
pub struct Oracle;

impl dlc_manager::Oracle for Oracle {
    fn get_public_key(&self) -> bitcoin::XOnlyPublicKey {
        unimplemented!()
    }

    fn get_attestation(
        &self,
        event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAttestation, dlc_manager::error::Error> {
        unimplemented!()
    }

    fn get_announcement(
        &self,
        event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAnnouncement, dlc_manager::error::Error> {
        unimplemented!()
    }
}
