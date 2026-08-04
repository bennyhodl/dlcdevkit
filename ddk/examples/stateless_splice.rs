//! Stateless DLC *splicing* — spending a previous contract's 2-of-2 funding
//! output as an input to a new contract (this is how rollovers and collateral
//! changes are expressed).
//!
//! Two things this example shows:
//!
//! 1. **Funding UTXOs are signed through the [`ddk_manager::Wallet`] trait**, exactly
//!    like `stateless_wallet.rs`. The wallet only ever sees the funding PSBT.
//!
//! 2. **Contract funding keys come from a [`ContractKeyProvider`]** — the
//!    deterministic "key generator". A contract's funding key is a pure function
//!    of its temporary id, so the caller never stores it: when splicing, each
//!    party re-derives the *previous* contract's funding key with
//!    [`ContractKeyProvider::dlc_input_signing_key`] and the prior temporary id.
//!    A [`ContractKeyProvider`] can be built from an xpriv, a seed, a BIP39
//!    mnemonic, or a private descriptor; in production
//!    [`ddk::wallet::DlcDevKitWallet`] is itself a provider.
//!
//! The splice is offer-only: the offering party (e.g. a borrower rolling a loan
//! over) contributes the splice input; the accepting party contributes no new
//! funds but still co-signs the prior 2-of-2 with its own recovered prior key.
//!
//! Run with `cargo run --example stateless_splice`.

#[allow(dead_code)]
mod util {
    include!("common/stateless.rs");
}

use bitcoin::bip32::Xpriv;
use bitcoin::psbt::Psbt;
use bitcoin::{Amount, Network, OutPoint, ScriptBuf};
use ddk::contract::{
    accept_offer, chain_hash_from_network, create_dlc_splice_input, create_funding_psbt,
    create_offer, finalize_sign, finalize_sign_spliced, funding_input, sign_accept,
    sign_accept_spliced, signing, AcceptOfferParams, ContractKeyProvider, CreateOfferParams, Party,
    PartyParams, DLC_INPUT_MAX_WITNESS_LEN,
};
use ddk_dlc::secp256k1_zkp::PublicKey;
use ddk_messages::FundingInput;

/// A minimal wallet implementing [`ddk_manager::Wallet`] over an in-memory BDK
/// wallet; only `sign_psbt_input` is exercised.
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

    fn utxo(&self, value: Amount, serial_id: u64) -> FundingInput {
        funding_input(
            &util::previous_transaction(value, self.script_pubkey.clone()),
            0,
            Some(serial_id),
            u32::MAX,
            108,
            ScriptBuf::new(),
        )
        .unwrap()
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

fn party_params(
    funding_pubkey: PublicKey,
    script_pubkey: ScriptBuf,
    funding_inputs: Vec<FundingInput>,
) -> PartyParams {
    PartyParams {
        funding_pubkey,
        funding_inputs,
        payout_spk: script_pubkey.clone(),
        payout_serial_id: None,
        change_spk: script_pubkey,
        change_serial_id: None,
    }
}

#[tokio::main]
async fn main() {
    let network = Network::Regtest;

    // Each party owns a wallet (signs funding UTXOs via the ddk_manager::Wallet
    // trait) and a contract-key provider (derives DLC funding keys, recoverably).
    let offerer_wallet = ExampleWallet::new(network, 71);
    let accepter_wallet = ExampleWallet::new(network, 72);
    let offerer_keys = ContractKeyProvider::from_seed(&[1u8; 64], network).unwrap();
    let accepter_keys = ContractKeyProvider::from_seed(&[2u8; 64], network).unwrap();

    // ----- Contract A: an ordinary dual-funded contract, fully signed. -----
    let temp_id_a = [0xA1; 32];
    let offer_a = create_offer(CreateOfferParams {
        chain_hash: chain_hash_from_network(network),
        temporary_contract_id: Some(temp_id_a),
        contract_info: util::enum_contract_info(util::TOTAL_COLLATERAL),
        offer_collateral: Amount::from_sat(50_000),
        party: party_params(
            offerer_keys.funding_pubkey(temp_id_a).unwrap(),
            offerer_wallet.script_pubkey.clone(),
            vec![offerer_wallet.utxo(Amount::from_sat(150_000), 1)],
        ),
        fund_output_serial_id: None,
        fee_rate_per_vb: 2,
        cet_locktime: 500,
        refund_locktime: 1_000,
        contract_flags: 0,
    })
    .expect("offer A");

    let accept_a = accept_offer(
        &offer_a,
        AcceptOfferParams {
            party: party_params(
                accepter_keys.funding_pubkey(temp_id_a).unwrap(),
                accepter_wallet.script_pubkey.clone(),
                vec![accepter_wallet.utxo(Amount::from_sat(150_000), 2)],
            ),
            min_timeout_interval: 100,
            max_timeout_interval: 500,
        },
        &accepter_keys.funding_secret_key(temp_id_a).unwrap(),
    )
    .expect("accept A")
    .accept;

    let mut offer_a_psbt = create_funding_psbt(&offer_a, &accept_a).unwrap();
    signing::sign_funding_psbt_with_wallet(
        &offer_a,
        &accept_a,
        &mut offer_a_psbt,
        &offerer_wallet,
        Party::Offer,
    )
    .await
    .expect("offer A wallet signing");
    let sign_a = sign_accept(
        &offer_a,
        &accept_a,
        &offerer_keys.funding_secret_key(temp_id_a).unwrap(),
        &offer_a_psbt,
    )
    .expect("sign A");

    let mut accept_a_psbt = create_funding_psbt(&offer_a, &accept_a).unwrap();
    signing::sign_funding_psbt_with_wallet(
        &offer_a,
        &accept_a,
        &mut accept_a_psbt,
        &accepter_wallet,
        Party::Accept,
    )
    .await
    .expect("accept A wallet signing");
    let funding_tx_a =
        finalize_sign(&offer_a, &accept_a, &sign_a.sign, &accept_a_psbt).expect("finalize A");

    // ----- Contract B: splice A's funding output into a new contract. -----
    let splice_serial = 900;
    let splice_input = create_dlc_splice_input(
        &offer_a,
        &accept_a,
        Party::Offer,
        Some(splice_serial),
        DLC_INPUT_MAX_WITNESS_LEN,
    )
    .expect("splice input");

    // Single-funded: the offering party rolls the old collateral in (the splice
    // input) plus a wallet UTXO of added collateral; the accepting party
    // contributes no new funds.
    let temp_id_b = [0xB2; 32];
    let splice_amount = Amount::from_sat(40_000);
    let offer_collateral_b = util::TOTAL_COLLATERAL + splice_amount;
    let offer_b = create_offer(CreateOfferParams {
        chain_hash: chain_hash_from_network(network),
        temporary_contract_id: Some(temp_id_b),
        contract_info: util::enum_contract_info(offer_collateral_b),
        offer_collateral: offer_collateral_b,
        party: party_params(
            offerer_keys.funding_pubkey(temp_id_b).unwrap(),
            offerer_wallet.script_pubkey.clone(),
            vec![
                splice_input,
                offerer_wallet.utxo(Amount::from_sat(200_000), 10),
            ],
        ),
        fund_output_serial_id: None,
        fee_rate_per_vb: 2,
        cet_locktime: 500,
        refund_locktime: 1_000,
        contract_flags: 0,
    })
    .expect("offer B");

    let accept_b = accept_offer(
        &offer_b,
        AcceptOfferParams {
            party: party_params(
                accepter_keys.funding_pubkey(temp_id_b).unwrap(),
                accepter_wallet.script_pubkey.clone(),
                vec![],
            ),
            min_timeout_interval: 100,
            max_timeout_interval: 500,
        },
        &accepter_keys.funding_secret_key(temp_id_b).unwrap(),
    )
    .expect("accept B")
    .accept;

    // Offer side: sign the new wallet UTXO, then produce this party's half of the
    // prior 2-of-2. The prior funding key is RE-DERIVED from `temp_id_a` — not
    // stored — via the provider's `dlc_input_signing_key` helper.
    let mut offer_b_psbt = create_funding_psbt(&offer_b, &accept_b).unwrap();
    signing::sign_funding_psbt_with_wallet(
        &offer_b,
        &accept_b,
        &mut offer_b_psbt,
        &offerer_wallet,
        Party::Offer,
    )
    .await
    .expect("offer B wallet signing");
    let offer_prior_key = offerer_keys
        .dlc_input_signing_key(temp_id_a, splice_serial)
        .expect("recover offer prior key");
    let sign_b = sign_accept_spliced(
        &offer_b,
        &accept_b,
        &offerer_keys.funding_secret_key(temp_id_b).unwrap(),
        &offer_b_psbt,
        std::slice::from_ref(&offer_prior_key),
    )
    .expect("sign B");

    // Accept side: no new inputs; contribute the other half of the prior 2-of-2,
    // again from the RE-DERIVED prior funding key.
    let accept_b_psbt = create_funding_psbt(&offer_b, &accept_b).unwrap();
    let accept_prior_key = accepter_keys
        .dlc_input_signing_key(temp_id_a, splice_serial)
        .expect("recover accept prior key");
    let funding_tx_b = finalize_sign_spliced(
        &offer_b,
        &accept_b,
        &sign_b.sign,
        &accept_b_psbt,
        std::slice::from_ref(&accept_prior_key),
    )
    .expect("finalize B");

    let spends_prior = funding_tx_b
        .input
        .iter()
        .any(|input| input.previous_output.txid == funding_tx_a.compute_txid());

    println!("prior  funding tx {}", funding_tx_a.compute_txid());
    println!(
        "splice funding tx {} ({} inputs, spends prior funding output: {})",
        funding_tx_b.compute_txid(),
        funding_tx_b.input.len(),
        spends_prior,
    );
    assert!(spends_prior, "splice must spend the prior funding output");
}
