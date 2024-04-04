use bdk::{chain::PersistBackend, wallet::ChangeSet};
use bdk_esplora::esplora_client::Error as EsploraError;
use dlc_manager::error::Error as ManagerError;

#[derive(Debug)]
enum ErnestWalletError {
    // Bdk(BdkError),
    Esplora(EsploraError),
}

// #[derive(Debug)]
// enum BdkError {
//     WriteError
// }
// impl<D: PersistBackend<ChangeSet>> From<D::WriteError> for ErnestWalletError {
//     fn from(value: D::WriteError) -> ErnestWalletError {
//         ErnestWalletError::Bdk(value)
//     }
// }

impl From<EsploraError> for ErnestWalletError {
    fn from(value: EsploraError) -> Self {
        ErnestWalletError::Esplora(value)
    }
}

impl From<ErnestWalletError> for ManagerError {
    fn from(e: ErnestWalletError) -> ManagerError {
        match e {
            // ErnestWalletError::Bdk(e) => ManagerError::WalletError(Box::new(e)),
            ErnestWalletError::Esplora(e) => ManagerError::BlockchainError(e.to_string()),
        }
    }
}

// pub fn bdk_err_to_manager_err(e: impl PersistBackend<ChangeSet>::WriteError) -> ManagerError {
//     ErnestWalletError::Bdk(e).into()
// }

pub fn esplora_err_to_manager_err(e: EsploraError) -> ManagerError {
    ErnestWalletError::Esplora(e).into()
}
