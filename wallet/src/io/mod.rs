use bdk::bitcoin::{util::bip32::ExtendedPrivKey, Network};
use getrandom::getrandom;
use std::path::{Path, PathBuf};

pub fn read_or_generate_xprv(
    wallet_name: &str,
    network: Network,
) -> anyhow::Result<ExtendedPrivKey> {
    let wallet_dir = get_ernest_dir().join(&wallet_name);

    let seed_file = &wallet_dir.join("seed");

    if Path::new(&seed_file).exists() {
        let seed = std::fs::read(seed_file)?;
        let mut key = [0; 78];
        key.copy_from_slice(&seed);

        let xprv = ExtendedPrivKey::decode(&key)?;

        Ok(xprv)
    } else {
        std::fs::create_dir_all(&wallet_dir)?;

        let mut entropy = [0u8; 78];

        getrandom(&mut entropy)?;

        // let _mnemonic = Mnemonic::from_entropy(&entropy)?;

        let xprv = ExtendedPrivKey::new_master(network, &entropy)?;

        std::fs::write(seed_file, &xprv.encode())?;

        Ok(xprv)
    }
}

pub fn get_ernest_dir() -> PathBuf {
    homedir::get_my_home()
        .unwrap()
        .unwrap()
        .join(format!(".ernest"))
}
