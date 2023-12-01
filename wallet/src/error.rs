use bdk::blockchain::esplora::EsploraError;
use dlc_manager::error::Error as ManagerError;

#[derive(Debug)]
enum ErnestWalletError {
    Bdk(bdk::Error),
    Esplora(bdk::blockchain::esplora::EsploraError),
}

impl From<bdk::Error> for ErnestWalletError {
    fn from(value: bdk::Error) -> ErnestWalletError {
        ErnestWalletError::Bdk(value)
    }
}

impl From<bdk::blockchain::esplora::EsploraError> for ErnestWalletError {
    fn from(value: bdk::blockchain::esplora::EsploraError) -> Self {
        ErnestWalletError::Esplora(value)
    }
}

impl From<ErnestWalletError> for ManagerError {
    fn from(e: ErnestWalletError) -> ManagerError {
        match e {
            ErnestWalletError::Bdk(e) => ManagerError::WalletError(Box::new(e)),
            ErnestWalletError::Esplora(e) => ManagerError::BlockchainError(e.to_string()),
        }
    }
}

pub fn bdk_err_to_manager_err(e: bdk::Error) -> ManagerError {
    ErnestWalletError::Bdk(e).into()
}

pub fn esplora_err_to_manager_err(e: EsploraError) -> ManagerError {
    ErnestWalletError::Esplora(e).into()
}
