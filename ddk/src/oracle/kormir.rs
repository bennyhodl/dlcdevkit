use bitcoin::key::XOnlyPublicKey;
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use std::str::FromStr;

const KORMIR_URL: &str = "http://localhost:8082";

fn get<T>(path: &str) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let url = format!("{}{}", KORMIR_URL, path);
    let request = reqwest::blocking::get(url)?.json::<T>()?;

    Ok(request)
}

#[derive(Debug)]
pub struct KormirOracleClient {
    pubkey: XOnlyPublicKey,
}

impl KormirOracleClient {
    pub fn new() -> anyhow::Result<KormirOracleClient> {
        let request: String = get("/pubkey")?;
        let pubkey = XOnlyPublicKey::from_str(&request)?;

        Ok(KormirOracleClient { pubkey })
    }

    pub fn get_pubkey(&self) -> anyhow::Result<XOnlyPublicKey> {
        let request: String = get("/pubkey")?;
        Ok(XOnlyPublicKey::from_str(&request)?)
    }
}

impl dlc_manager::Oracle for KormirOracleClient {
    fn get_public_key(&self) -> bitcoin::key::XOnlyPublicKey {
        self.pubkey
    }

    fn get_attestation(
        &self,
        _event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAttestation, dlc_manager::error::Error> {
        get::<OracleAttestation>("attestation").map_err(|_| dlc_manager::error::Error::OracleError("Could not get attestation".into()))
    }

    fn get_announcement(
        &self,
        _event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAnnouncement, dlc_manager::error::Error> {
        get::<OracleAnnouncement>("announcement").map_err(|_| {
            dlc_manager::error::Error::OracleError("Could not get announcement".into())
        })
    }
}

impl crate::DdkOracle for KormirOracleClient {
    fn name(&self) -> String {
        "kormir".into()
    }
}
