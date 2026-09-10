use bitcoin::key::rand;
use rand::Fill;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

/// The seed file holds the root of every key the node uses, so only its owner
/// may read it.
#[cfg(unix)]
const SEED_FILE_MODE: u32 = 0o600;

/// Helper function that reads `[bitcoin::bip32::Xpriv]` bytes from a file.
/// If the file does not exist then it will create a file `seed.ddk` in the specified path.
///
/// The file is created with owner-only permissions. An existing file with
/// wider permissions is tightened on read, and a file that is not exactly 64
/// bytes is an error rather than a panic.
pub fn xprv_from_path(path: PathBuf) -> anyhow::Result<[u8; 64]> {
    let seed_path = path.join("seed.ddk");
    let seed = if Path::new(&seed_path).exists() {
        restrict_permissions(&seed_path)?;
        let seed = std::fs::read(&seed_path)?;
        let key: [u8; 64] = seed.as_slice().try_into().map_err(|_| {
            anyhow::anyhow!(
                "seed file {} holds {} bytes, expected 64; restore the original file before starting",
                seed_path.display(),
                seed.len()
            )
        })?;
        key
    } else {
        let mut file = create_private_file(&seed_path)?;
        let mut entropy = [0u8; 64];
        entropy.try_fill(&mut rand::thread_rng())?;
        file.write_all(&entropy)?;
        file.sync_all()?;
        entropy
    };

    Ok(seed)
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> anyhow::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(SEED_FILE_MODE)
        .open(path)?)
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> anyhow::Result<File> {
    Ok(OpenOptions::new().write(true).create_new(true).open(path)?)
}

/// Removes group and world access from an existing seed file.
#[cfg(unix)]
fn restrict_permissions(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path)?;
    let mut permissions = metadata.permissions();
    if permissions.mode() & 0o077 != 0 {
        permissions.set_mode(SEED_FILE_MODE);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ddk-node-seed-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_new_seed_file_is_owner_only_and_reads_back() {
        let dir = temp_dir("new");
        let seed = xprv_from_path(dir.clone()).unwrap();
        let mode = std::fs::metadata(dir.join("seed.ddk"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_ne!(seed, [0u8; 64]);
        assert_eq!(xprv_from_path(dir).unwrap(), seed);
    }

    #[test]
    fn an_open_seed_file_is_tightened_on_read() {
        let dir = temp_dir("open");
        let seed_path = dir.join("seed.ddk");
        std::fs::write(&seed_path, [7u8; 64]).unwrap();
        std::fs::set_permissions(&seed_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(xprv_from_path(dir).unwrap(), [7u8; 64]);
        let mode = std::fs::metadata(&seed_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_wrong_length_seed_file_is_an_error() {
        let dir = temp_dir("short");
        std::fs::write(dir.join("seed.ddk"), [1u8; 63]).unwrap();
        let error = xprv_from_path(dir).unwrap_err().to_string();
        assert!(error.contains("63 bytes"), "{error}");
    }
}
