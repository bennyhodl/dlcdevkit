use bitcoin::key::XOnlyPublicKey;
use dlc_messages::oracle_msgs::OracleAnnouncement;
use std::str::FromStr;

const ORACLE_URL: &str = "http://localhost:8082";

fn get<T>(path: &str) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let url = format!("{}{}", ORACLE_URL, path);
    let request = reqwest::blocking::get(url)?.json::<T>()?;

    Ok(request)
}

#[derive(Debug)]
pub struct DlcDevKitOracle {
    pubkey: XOnlyPublicKey,
}

impl DlcDevKitOracle {
    pub fn new() -> anyhow::Result<DlcDevKitOracle> {
        let request: String = get("/pubkey")?;
        let pubkey = XOnlyPublicKey::from_str(&request)?;

        Ok(DlcDevKitOracle { pubkey })
    }

    pub fn get_pubkey(&self) -> anyhow::Result<XOnlyPublicKey> {
        let request: String = get("/pubkey")?;
        Ok(XOnlyPublicKey::from_str(&request)?)
    }
}

impl dlc_manager::Oracle for DlcDevKitOracle {
    fn get_public_key(&self) -> bitcoin::key::XOnlyPublicKey {
        self.pubkey
    }

    fn get_attestation(
        &self,
        _event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAttestation, dlc_manager::error::Error> {
        // get::<String>("attestation").map_err(|_| dlc_manager::error::Error::OracleError("Could not get attestation".into()))
        unimplemented!()
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

impl crate::DdkOracle for DlcDevKitOracle {}
