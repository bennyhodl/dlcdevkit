use crate::wallet::{Address, WalletError};

/// Custom Wallet Generator
///
/// Some application might want to have control of how contract addresses are generated.
/// The default behavior is the wallet generates an address in the BIP84 template (84'/0'/0'/{0,1}/*)
/// if an application wants to control the derivation path, it can implement this trait and pass it to the wallet.
#[async_trait::async_trait]
pub trait AddressGenerator: Send + Sync + 'static {
    async fn custom_external_address(&self) -> Result<Address, WalletError>;
    async fn custom_change_address(&self) -> Result<Address, WalletError>;
}
