use bdk::bitcoin::{util::bip32::ExtendedPrivKey, Network};
use getrandom::getrandom;
use std::path::{Path, PathBuf};

pub fn read_or_generate_xprv(
    seed_path: PathBuf,
    network: Network,
) -> anyhow::Result<ExtendedPrivKey> {
    if Path::new(&seed_path).exists() {
        let seed = std::fs::read(seed_path)?;
        let mut key = [0; 78];
        key.copy_from_slice(&seed);

        let xprv = ExtendedPrivKey::decode(&key)?;

        Ok(xprv)
    } else {
        let mut entropy = [0u8; 78];

        getrandom(&mut entropy)?;

        // let _mnemonic = Mnemonic::from_entropy(&entropy)?;

        let xprv = ExtendedPrivKey::new_master(network, &entropy)?;

        std::fs::write(seed_path, &xprv.encode())?;

        Ok(xprv)
    }
}

pub fn create_ernest_dir_with_wallet(wallet_name: String) -> anyhow::Result<PathBuf> {
    let dir = homedir::get_my_home()?.unwrap().join(".ernest");
    std::fs::create_dir_all(dir.clone())?;

    let file = dir.join(wallet_name);
    Ok(file)
}

pub fn get_ernest_dir() -> PathBuf {
    homedir::get_my_home()
        .unwrap()
        .unwrap()
        .join(format!(".ernest"))
}
