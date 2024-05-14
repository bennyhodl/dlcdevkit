use bdk_esplora::esplora_client::Error as EsploraError;
use dlc_manager::error::Error as ManagerError;

#[derive(Debug)]
enum DlcDevKitError {
    // Bdk(BdkError),
    Esplora(EsploraError),
}

// #[derive(Debug)]
// enum BdkError {
//     WriteError
// }
// impl<D: PersistBackend<ChangeSet>> From<D::WriteError> for DlcDevKitError {
//     fn from(value: D::WriteError) -> DlcDevKitError {
//         DlcDevKitError::Bdk(value)
//     }
// }

impl From<EsploraError> for DlcDevKitError {
    fn from(value: EsploraError) -> Self {
        DlcDevKitError::Esplora(value)
    }
}

impl From<DlcDevKitError> for ManagerError {
    fn from(e: DlcDevKitError) -> ManagerError {
        match e {
            // DlcDevKitError::Bdk(e) => ManagerError::WalletError(Box::new(e)),
            DlcDevKitError::Esplora(e) => ManagerError::BlockchainError(e.to_string()),
        }
    }
}

// pub fn bdk_err_to_manager_err(e: impl PersistBackend<ChangeSet>::WriteError) -> ManagerError {
//     DlcDevKitError::Bdk(e).into()
// }

pub fn esplora_err_to_manager_err(e: EsploraError) -> ManagerError {
    DlcDevKitError::Esplora(e).into()
}
