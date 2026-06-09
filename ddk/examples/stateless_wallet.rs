//! Stateless DLC lifecycle with funding inputs signed by a wallet
//! implementing [`ddk_manager::Wallet`].
//!
//! The wallet only ever sees the funding PSBT — no contract manager, signer
//! provider, or storage trait is involved. Any wallet that can sign PSBT
//! inputs works, including [`ddk::wallet::DlcDevKitWallet`]; this example uses
//! a minimal in-memory BDK wallet.
//!
//! Run with `cargo run --example stateless_wallet`.

#[allow(dead_code)]
mod util {
    include!("common/stateless.rs");
}

use bitcoin::bip32::Xpriv;
use bitcoin::psbt::Psbt;
use bitcoin::{Amount, Network, OutPoint, ScriptBuf};
use ddk::contract::{
    accept_offer, create_funding_psbt, create_offer, finalize_sign, funding_input, sign_accept,
    signing, AcceptOfferParams, Party,
};
use ddk_dlc::secp256k1_zkp::Secp256k1;
use util::PartySetup;

/// A minimal wallet implementing [`ddk_manager::Wallet`] over an in-memory
/// BDK wallet. Only `sign_psbt_input` is used by the stateless API.
struct ExampleWallet {
    wallet: std::sync::Mutex<bdk_wallet::Wallet>,
    script_pubkey: ScriptBuf,
}

impl ExampleWallet {
    fn new(network: Network, seed_byte: u8) -> Self {
        let xpriv = Xpriv::new_master(network, &[seed_byte; 64]).unwrap();
        let descriptor = format!("wpkh({xpriv}/84h/1h/0h/0/*)");
        let mut wallet = bdk_wallet::Wallet::create_single(descriptor)
            .network(network)
            .create_wallet_no_persist()
            .unwrap();
        let address = wallet.reveal_next_address(bdk_wallet::KeychainKind::External);
        Self {
            wallet: std::sync::Mutex::new(wallet),
            script_pubkey: address.address.script_pubkey(),
        }
    }
}

#[async_trait::async_trait]
impl ddk_manager::Wallet for ExampleWallet {
    async fn get_new_address(&self) -> Result<bitcoin::Address, ddk_manager::error::Error> {
        unimplemented!("not needed for PSBT signing")
    }
    async fn get_new_change_address(&self) -> Result<bitcoin::Address, ddk_manager::error::Error> {
        unimplemented!("not needed for PSBT signing")
    }
    async fn get_utxos_for_amount(
        &self,
        _amount: Amount,
        _fee_rate: u64,
        _lock_utxos: bool,
    ) -> Result<Vec<ddk_manager::Utxo>, ddk_manager::error::Error> {
        unimplemented!("not needed for PSBT signing")
    }
    async fn sign_psbt_input(
        &self,
        psbt: &mut Psbt,
        input_index: usize,
    ) -> Result<(), ddk_manager::error::Error> {
        let wallet = self.wallet.lock().unwrap();
        let mut signed = psbt.clone();
        let options = bdk_wallet::SignOptions {
            trust_witness_utxo: true,
            ..Default::default()
        };
        wallet
            .sign(&mut signed, options)
            .map_err(|e| ddk_manager::error::Error::WalletError(Box::new(e)))?;
        psbt.inputs[input_index] = signed.inputs[input_index].clone();
        Ok(())
    }
    fn import_address(&self, _address: &bitcoin::Address) -> Result<(), ddk_manager::error::Error> {
        Ok(())
    }
    fn unreserve_utxos(&self, _outpoints: &[OutPoint]) -> Result<(), ddk_manager::error::Error> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let secp = Secp256k1::new();
    let network = Network::Regtest;

    let offerer_wallet = ExampleWallet::new(network, 71);
    let accepter_wallet = ExampleWallet::new(network, 72);

    // DLC funding keys stay with the application; the wallets only control
    // the UTXOs spent into the funding transaction.
    let offerer = PartySetup::new(&secp, 1, network, Amount::from_sat(150_000));
    let accepter = PartySetup::new(&secp, 2, network, Amount::from_sat(150_000));
    let offer_input = funding_input(
        &util::previous_transaction(
            Amount::from_sat(150_000),
            offerer_wallet.script_pubkey.clone(),
        ),
        0,
        Some(1),
        u32::MAX,
        108,
        ScriptBuf::new(),
    )
    .unwrap();
    let accept_input = funding_input(
        &util::previous_transaction(
            Amount::from_sat(150_000),
            accepter_wallet.script_pubkey.clone(),
        ),
        0,
        Some(2),
        u32::MAX,
        108,
        ScriptBuf::new(),
    )
    .unwrap();

    let offer = create_offer(util::offer_params_with_party(
        offerer.party_params_with_inputs(&secp, vec![offer_input]),
        Amount::from_sat(50_000),
        network,
    ))
    .expect("valid offer");

    let accept_result = accept_offer(
        &offer,
        AcceptOfferParams {
            party: accepter.party_params_with_inputs(&secp, vec![accept_input]),
            min_timeout_interval: 100,
            max_timeout_interval: 500,
        },
        &accepter.funding_secret_key,
    )
    .expect("valid accept");
    let accept = accept_result.accept;

    let mut offer_psbt = create_funding_psbt(&offer, &accept).expect("funding psbt");
    signing::sign_funding_psbt_with_wallet(
        &offer,
        &accept,
        &mut offer_psbt,
        &offerer_wallet,
        Party::Offer,
    )
    .await
    .expect("offer wallet signing");
    let sign_result =
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &offer_psbt).expect("sign");

    let mut accept_psbt = create_funding_psbt(&offer, &accept).expect("funding psbt");
    signing::sign_funding_psbt_with_wallet(
        &offer,
        &accept,
        &mut accept_psbt,
        &accepter_wallet,
        Party::Accept,
    )
    .await
    .expect("accept wallet signing");
    let funding_transaction =
        finalize_sign(&offer, &accept, &sign_result.sign, &accept_psbt).expect("finalize");

    println!(
        "completed funding transaction {} with {} signed inputs",
        funding_transaction.compute_txid(),
        funding_transaction.input.len()
    );
}
