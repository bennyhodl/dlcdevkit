//! Scaffolding for the stateless contract execution tests.
//!
//! This mirrors `ddk-manager/tests/test_utils.rs`: a live regtest chain
//! (a bitcoind and an electrs that [`ChainContext`] starts through
//! [`ddk_testenv`]), real funded UTXOs, and real Kormir oracles. What it
//! does *not* mirror is state — no [`ddk_manager::manager::Manager`], no
//! [`ddk_manager::Storage`], and no persisted contract exists anywhere in these
//! tests. Every step goes through [`ddk::contract`] and the wire messages it
//! produces, and the resulting transactions are broadcast to a real node.
//!
//! The pieces a scenario composes are:
//!
//! | Concern | Type |
//! |---------|------|
//! | chain access | [`ChainContext`] |
//! | oracles and attestations | [`TestOracles`] |
//! | a party's keys, UTXOs, and signer | [`TestParty`] |
//! | offer → accept → sign → broadcast | [`fund_contract`] |
//! | CET / refund settlement | [`close_with_cet`], [`close_with_refund`] |

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::hashes::{sha256, Hash};
use bitcoin::psbt::Psbt;
use bitcoin::{Address, Amount, Network, OutPoint, ScriptBuf, Transaction, Txid, Witness};
use bitcoincore_rpc::{Client, RpcApi};
use ddk::chain::EsploraClient;
use ddk::contract::{
    accept_offer, chain_hash_from_network, create_dlc_splice_input, create_dlc_transactions,
    create_funding_psbt, create_offer, finalize_sign_spliced, funding_input, sign_accept_spliced,
    sign_cet, sign_refund, signing, AcceptOfferParams, ContractError, ContractKeyProvider,
    CreateOfferParams, DescriptorInput, DlcInputSigningKey, InputDerivation, Party, PartyParams,
    DLC_INPUT_MAX_WITNESS_LEN,
};
use ddk::logger::Logger;
use ddk::oracle::memory::MemoryOracle;
use ddk::storage::memory::MemoryStorage;
use ddk::wallet::DlcDevKitWallet;
use ddk_dlc::secp256k1_zkp::{All, PublicKey, Secp256k1, SecretKey};
use ddk_dlc::DlcTransactions;
use ddk_manager::contract::numerical_descriptor::DifferenceParams;
use ddk_manager::Blockchain;
use ddk_messages::contract_msgs::ContractInfo;
use ddk_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use ddk_messages::{AcceptDlc, FundingInput, OfferDlc, SignDlc};
use ddk_testenv::dlc;
use ddk_testenv::TestEnv;
use std::str::FromStr;

// Oracles, events and contract descriptors are the same in both DLC test
// suites, so they come from [`ddk_testenv::dlc`] rather than from here.
pub use ddk_testenv::dlc::{
    difference_params, max_value as max_numeric_value, SpliceDelta, EVENT_MATURITY, NB_DIGITS,
};

/// The chain every test runs against.
pub const NETWORK: Network = Network::Regtest;

/// Distance between the oracle maturity and the refund locktime.
pub const REFUND_DELAY: u32 = 604_800;

/// The accepting party's timeout policy, satisfied exactly by [`REFUND_DELAY`].
pub const MIN_TIMEOUT_INTERVAL: u32 = REFUND_DELAY;
pub const MAX_TIMEOUT_INTERVAL: u32 = REFUND_DELAY;

pub const OFFER_COLLATERAL: Amount = Amount::from_sat(1_000_000);
pub const ACCEPT_COLLATERAL: Amount = Amount::from_sat(1_000_000);
pub const TOTAL_COLLATERAL: Amount = Amount::from_sat(2_000_000);

/// Value of each UTXO funded into a party's wallet.
pub const UTXO_VALUE: Amount = Amount::from_sat(5_000_000);

pub const FEE_RATE_PER_VB: u64 = 2;

/// Ordinary P2WPKH funding inputs carry a two-element witness.
pub const P2WPKH_MAX_WITNESS_LEN: u16 = 108;

const BIP84_ACCOUNT: &str = "84h/1h/0h";
const PAYOUT_INDEX: u32 = 100;
const CHANGE_INDEX: u32 = 101;

/// Two distinct BIP39 test vectors so mnemonic-derived parties get distinct keys.
const OFFER_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const ACCEPT_MNEMONIC: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";

/// A deterministic temporary contract id derived from a test label.
///
/// Both parties derive their DLC funding keys from this id, so it has to be
/// fixed before either party is built.
pub fn temporary_contract_id(label: &str) -> [u8; 32] {
    sha256::Hash::hash(label.as_bytes()).to_byte_array()
}

/// Where a party's *wallet input* signatures come from.
///
/// Each variant drives a different [`ddk::contract::signing`] entry point over
/// the same funding PSBT; the contract logic never learns which was used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputSource {
    /// [`signing::sign_funding_psbt_with_xpriv`] with explicit BIP32 paths.
    Xpriv,
    /// [`signing::sign_funding_psbt_with_descriptor`] with a private `wpkh()` descriptor.
    Descriptor,
    /// [`signing::sign_funding_psbt_with_wallet`] backed by a real [`DlcDevKitWallet`].
    DdkWallet,
    /// No DDK code at all: the PSBT is serialized, signed and finalized with
    /// plain rust-bitcoin, then handed back.
    ExternalSigner,
}

/// Where a party's *DLC funding key* comes from.
///
/// Every variant other than [`FundingKeySource::RawSecretKey`] goes through
/// [`ContractKeyProvider`], so the key is a pure function of the contract's
/// temporary id and can be recomputed later (which is what splicing needs).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FundingKeySource {
    /// A key the application holds directly.
    RawSecretKey,
    /// [`ContractKeyProvider::from_xprv`].
    Xprv,
    /// [`ContractKeyProvider::from_seed`].
    Seed,
    /// [`ContractKeyProvider::from_mnemonic`].
    Mnemonic,
    /// [`ContractKeyProvider::from_descriptor`].
    Descriptor,
}

/// Access to the regtest chain: bitcoind for funding and mining, Esplora for
/// broadcasting and confirmation checks.
pub struct ChainContext {
    pub esplora: Arc<EsploraClient>,
    pub sink: Client,
    pub logger: Arc<Logger>,
    pub network: Network,
    /// Held for its [`Drop`]: the bitcoind and electrs children die with it.
    /// Declared last so the clients above are torn down before the nodes are.
    _env: TestEnv,
}

impl ChainContext {
    /// Boots a bitcoind and an electrs used by this context alone.
    ///
    /// Private rather than shared, matching
    /// `ddk-manager/tests/test_utils.rs::test_env`: these tests mine to reach
    /// confirmation depths and settle against locktimes, so blocks a sibling
    /// test mined on a shared chain would move the tip out from under them.
    /// [`TestEnv`] leaves the chain past coinbase maturity, so it can fund a
    /// party immediately.
    pub async fn new(name: &str) -> Self {
        let env = TestEnv::new();
        let logger = Arc::new(Logger::disabled(name.to_string()));
        let esplora = Arc::new(
            EsploraClient::new(env.esplora_host(), NETWORK, logger.clone())
                .expect("could not build the Esplora client"),
        );
        Self {
            esplora,
            sink: env.rpc(),
            logger,
            network: NETWORK,
            _env: env,
        }
    }

    /// Mines blocks and waits for Esplora to catch up to the new tip.
    pub async fn generate_blocks(&self, nb_blocks: u32) {
        let previous_height = self.esplora.async_client.get_height().await.unwrap();
        let sink_address = self
            .sink
            .get_new_address(None, None)
            .expect("RPC error")
            .assume_checked();
        self.sink
            .generate_to_address(nb_blocks as u64, &sink_address)
            .expect("RPC error");

        let target_height = previous_height + nb_blocks;
        let mut attempts = 0;
        loop {
            if self.esplora.async_client.get_height().await.unwrap() >= target_height {
                return;
            }
            attempts += 1;
            assert!(
                attempts < 150,
                "Esplora did not reach height {target_height}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Pays `amount` to `script_pubkey`, confirms it, and returns the funding
    /// transaction with the index of the matching output.
    pub async fn fund_script(
        &self,
        script_pubkey: &ScriptBuf,
        amount: Amount,
    ) -> (Transaction, u32) {
        let address = Address::from_script(script_pubkey, self.network)
            .expect("script pubkey is not a valid address");
        let txid = self
            .sink
            .send_to_address(&address, amount, None, None, None, None, None, None)
            .expect("RPC error");
        self.generate_blocks(3).await;
        let transaction = self
            .sink
            .get_raw_transaction(&txid, None)
            .expect("RPC error");
        let vout = transaction
            .output
            .iter()
            .position(|output| output.script_pubkey == *script_pubkey)
            .expect("funding transaction does not pay the requested script")
            as u32;
        self.wait_for_confirmation(&txid).await;
        (transaction, vout)
    }

    /// Broadcasts a transaction, failing the test with the node's reason if it
    /// is rejected.
    pub async fn broadcast(&self, transaction: &Transaction) {
        self.esplora
            .send_transaction(transaction)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "node rejected transaction {}: {e}",
                    transaction.compute_txid()
                )
            });
    }

    /// Broadcasts a transaction and mines until it is `nb_blocks` deep.
    ///
    /// The depth is read back from Esplora rather than assumed, and topped up
    /// until the target is reached: a transaction that does not make it into
    /// the next block is one confirmation short of where the block count says
    /// it should be.
    pub async fn broadcast_and_confirm(&self, transaction: &Transaction, nb_blocks: u32) {
        self.broadcast(transaction).await;
        let txid = transaction.compute_txid();
        self.generate_blocks(nb_blocks).await;
        self.wait_for_confirmation(&txid).await;
        for _ in 0..30 {
            let confirmations = self
                .esplora
                .get_transaction_confirmations(&txid)
                .await
                .unwrap_or(0);
            if confirmations >= nb_blocks {
                return;
            }
            self.generate_blocks(nb_blocks - confirmations).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        panic!("transaction {txid} never reached {nb_blocks} confirmations");
    }

    async fn wait_for_confirmation(&self, txid: &Txid) {
        for _ in 0..150 {
            if self
                .esplora
                .get_transaction_confirmations(txid)
                .await
                .unwrap_or(0)
                > 0
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        panic!("transaction {txid} never confirmed");
    }

    /// Asserts the chain records `outpoint` as spent by `spender`.
    pub async fn assert_spent_by(&self, outpoint: OutPoint, spender: &Transaction) {
        let status = self
            .esplora
            .async_client
            .get_output_status(&outpoint.txid, outpoint.vout as u64)
            .await
            .expect("Esplora error")
            .expect("unknown output");
        assert_eq!(
            status.txid,
            Some(spender.compute_txid()),
            "{outpoint} was not spent by the expected transaction"
        );
    }
}

/// A set of oracles that have all announced the same event.
pub struct TestOracles {
    pub oracles: Vec<MemoryOracle>,
    pub announcements: Vec<OracleAnnouncement>,
    pub threshold: u16,
    pub event_id: String,
}

impl TestOracles {
    /// Creates `nb_oracles` oracles announcing the same enum event.
    pub async fn enums(nb_oracles: usize, threshold: u16, event_id: &str) -> Self {
        let oracles = dlc::new_oracles(nb_oracles);
        let announcements = dlc::announce_enum_event(&oracles, event_id, EVENT_MATURITY).await;
        Self {
            oracles,
            announcements,
            threshold,
            event_id: event_id.to_string(),
        }
    }

    /// Creates `nb_oracles` oracles announcing the same digit decomposition event.
    pub async fn numerics(nb_oracles: usize, threshold: u16, event_id: &str) -> Self {
        let oracles = dlc::new_oracles(nb_oracles);
        let announcements = dlc::announce_numeric_event(
            &oracles,
            event_id,
            &vec![NB_DIGITS as usize; nb_oracles],
            EVENT_MATURITY,
        )
        .await;
        Self {
            oracles,
            announcements,
            threshold,
            event_id: event_id.to_string(),
        }
    }

    /// The oracles that settle a contract: the first `threshold` of them.
    ///
    /// Which oracles sign is this suite's own policy — the manager tests pick a
    /// random subset above the threshold instead.
    fn signers(&self) -> Vec<usize> {
        (0..self.threshold as usize).collect()
    }

    /// Attests `outcome` with the settling oracles and returns the attestations
    /// paired with their index in [`Self::announcements`].
    pub async fn attest_enum(&self, outcome: &str) -> Vec<(usize, OracleAttestation)> {
        let signers = self.signers();
        dlc::sign_enum_event(&self.oracles, &self.event_id, &signers, outcome).await;
        dlc::attestations(&self.oracles, &self.event_id, &signers).await
    }

    /// Attests a numeric outcome with the settling oracles.
    ///
    /// When `spread` is set the oracles after the first alternate one unit
    /// either side of `outcome`, which is the disagreement a contract with
    /// difference params is built to tolerate.
    pub async fn attest_numeric(
        &self,
        outcome: i64,
        spread: bool,
    ) -> Vec<(usize, OracleAttestation)> {
        let signers = self.signers();
        for index in &signers {
            let signed_outcome = if spread && *index > 0 {
                if index % 2 == 0 {
                    outcome + 1
                } else {
                    outcome - 1
                }
            } else {
                outcome
            };
            dlc::sign_numeric_event(
                &self.oracles,
                &self.event_id,
                std::slice::from_ref(index),
                signed_outcome,
            )
            .await;
        }
        dlc::attestations(&self.oracles, &self.event_id, &signers).await
    }

    /// One event of a contract: `descriptor`, settled by these oracles.
    fn leg(&self, descriptor: ddk_manager::contract::ContractDescriptor) -> dlc::ContractLeg {
        dlc::ContractLeg::new(descriptor, self.announcements.clone(), self.threshold)
    }
}

/// The payout curve every numeric contract in this suite runs on: a straight
/// line from nothing to the whole collateral over the first 900 outcomes.
fn numeric_descriptor(
    oracles: &TestOracles,
    offer_collateral: Amount,
    accept_collateral: Amount,
    difference_params: Option<DifferenceParams>,
) -> ddk_manager::contract::ContractDescriptor {
    let payout_function = ddk_payouts::generate_payout_curve(
        0,
        900,
        offer_collateral,
        accept_collateral,
        5,
        max_numeric_value(),
    )
    .unwrap();
    dlc::numeric_descriptor(
        payout_function,
        dlc::numeric_infos(oracles.announcements.len()),
        difference_params,
    )
}

/// A single-event enum contract.
pub fn enum_contract_info(oracles: &TestOracles, total_collateral: Amount) -> ContractInfo {
    dlc::contract_info(
        &[oracles.leg(dlc::enum_descriptor(total_collateral))],
        total_collateral,
    )
}

/// A single-event numeric contract, optionally tolerating oracle disagreement.
pub fn numeric_contract_info(
    oracles: &TestOracles,
    offer_collateral: Amount,
    accept_collateral: Amount,
    difference_params: Option<DifferenceParams>,
) -> ContractInfo {
    let descriptor = numeric_descriptor(
        oracles,
        offer_collateral,
        accept_collateral,
        difference_params,
    );
    dlc::contract_info(
        &[oracles.leg(descriptor)],
        offer_collateral + accept_collateral,
    )
}

/// A disjoint contract: either the enum event or the numeric event can settle it.
pub fn disjoint_contract_info(
    enum_oracles: &TestOracles,
    numeric_oracles: &TestOracles,
    offer_collateral: Amount,
    accept_collateral: Amount,
) -> ContractInfo {
    let total_collateral = offer_collateral + accept_collateral;
    let numeric = numeric_descriptor(numeric_oracles, offer_collateral, accept_collateral, None);
    dlc::contract_info(
        &[
            enum_oracles.leg(dlc::enum_descriptor(total_collateral)),
            numeric_oracles.leg(numeric),
        ],
        total_collateral,
    )
}

/// The enum outcome a scenario settles on. Index 0 of [`enum_outcomes`], so it
/// pays the offering party the whole of the collateral.
pub const SETTLEMENT_OUTCOME: &str = "a";

/// The numeric outcome a scenario settles on.
///
/// Well inside the payout curve, so the spread a contract with difference
/// params tolerates stays in range on both sides.
pub const SETTLEMENT_VALUE: i64 = 500;

/// Which event of a disjoint contract settles it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisjointEvent {
    Enum,
    Numeric,
}

/// A contract shape, with the collateral left out: which events settle it, over
/// how many oracles, and how many of them have to agree.
///
/// A splice chain builds a contract per round, each with its own oracles and
/// its own collateral. A scenario names the shape once and every round
/// instantiates it through [`ShapedContract::new`].
#[derive(Clone, Copy, Debug)]
pub enum ContractShape {
    Enum {
        nb_oracles: usize,
        threshold: u16,
    },
    Numeric {
        nb_oracles: usize,
        threshold: u16,
        with_difference: bool,
    },
    /// An enum event and a numeric event, either of which settles the contract;
    /// `settle_on` names the one the scenario attests.
    Disjoint {
        nb_oracles: usize,
        threshold: u16,
        settle_on: DisjointEvent,
    },
}

impl ContractShape {
    pub fn enums(nb_oracles: usize, threshold: u16) -> Self {
        ContractShape::Enum {
            nb_oracles,
            threshold,
        }
    }

    pub fn numeric(nb_oracles: usize, threshold: u16) -> Self {
        ContractShape::Numeric {
            nb_oracles,
            threshold,
            with_difference: false,
        }
    }

    /// A numeric contract that tolerates the oracles disagreeing by a bounded
    /// amount, which only means anything above one oracle.
    pub fn numeric_with_difference(nb_oracles: usize, threshold: u16) -> Self {
        ContractShape::Numeric {
            nb_oracles,
            threshold,
            with_difference: true,
        }
    }

    pub fn disjoint(nb_oracles: usize, threshold: u16, settle_on: DisjointEvent) -> Self {
        ContractShape::Disjoint {
            nb_oracles,
            threshold,
            settle_on,
        }
    }
}

/// A [`ContractShape`] instantiated over its own oracles, for one collateral.
///
/// It holds the oracles it announced with, so the scenario that funded the
/// contract can attest the event afterwards without tracking them itself.
pub struct ShapedContract {
    pub contract_info: ContractInfo,
    enum_oracles: Option<TestOracles>,
    numeric_oracles: Option<TestOracles>,
    with_difference: bool,
    settle_on: DisjointEvent,
}

impl ShapedContract {
    /// Announces the events `shape` needs under event ids derived from `label`,
    /// and builds the contract info locking
    /// `offer_collateral + accept_collateral`.
    ///
    /// The split between the two collaterals only reaches the payout curve of a
    /// numeric contract; every shape locks their sum.
    pub async fn new(
        shape: ContractShape,
        label: &str,
        offer_collateral: Amount,
        accept_collateral: Amount,
    ) -> Self {
        match shape {
            ContractShape::Enum {
                nb_oracles,
                threshold,
            } => {
                let oracles = TestOracles::enums(nb_oracles, threshold, label).await;
                let contract_info =
                    enum_contract_info(&oracles, offer_collateral + accept_collateral);
                Self {
                    contract_info,
                    enum_oracles: Some(oracles),
                    numeric_oracles: None,
                    with_difference: false,
                    settle_on: DisjointEvent::Enum,
                }
            }
            ContractShape::Numeric {
                nb_oracles,
                threshold,
                with_difference,
            } => {
                let oracles = TestOracles::numerics(nb_oracles, threshold, label).await;
                let contract_info = numeric_contract_info(
                    &oracles,
                    offer_collateral,
                    accept_collateral,
                    with_difference.then(difference_params),
                );
                Self {
                    contract_info,
                    enum_oracles: None,
                    numeric_oracles: Some(oracles),
                    with_difference,
                    settle_on: DisjointEvent::Numeric,
                }
            }
            ContractShape::Disjoint {
                nb_oracles,
                threshold,
                settle_on,
            } => {
                let enum_oracles =
                    TestOracles::enums(nb_oracles, threshold, &format!("{label}-enum")).await;
                let numeric_oracles =
                    TestOracles::numerics(nb_oracles, threshold, &format!("{label}-numeric")).await;
                let contract_info = disjoint_contract_info(
                    &enum_oracles,
                    &numeric_oracles,
                    offer_collateral,
                    accept_collateral,
                );
                Self {
                    contract_info,
                    enum_oracles: Some(enum_oracles),
                    numeric_oracles: Some(numeric_oracles),
                    with_difference: false,
                    settle_on,
                }
            }
        }
    }

    /// Attests the event this contract settles on, with as many oracles as its
    /// threshold asks for.
    pub async fn attest(&self) -> Vec<(usize, OracleAttestation)> {
        match self.settle_on {
            DisjointEvent::Enum => {
                self.enum_oracles
                    .as_ref()
                    .expect("a contract settling on an enum event to have enum oracles")
                    .attest_enum(SETTLEMENT_OUTCOME)
                    .await
            }
            DisjointEvent::Numeric => {
                self.numeric_oracles
                    .as_ref()
                    .expect("a contract settling on a numeric event to have numeric oracles")
                    .attest_numeric(SETTLEMENT_VALUE, self.with_difference)
                    .await
            }
        }
    }
}

/// A funding input together with the derivation index of the key controlling it.
pub struct PartyInput {
    pub funding_input: FundingInput,
    pub derivation_index: u32,
}

/// One side of a contract: its DLC funding key, its on-chain UTXOs, and the
/// signer that will sign them.
pub struct TestParty {
    pub role: Party,
    pub input_source: InputSource,
    pub funding_key_source: FundingKeySource,
    pub xpriv: Xpriv,
    pub contract_keys: Option<ContractKeyProvider>,
    pub funding_secret_key: SecretKey,
    pub inputs: Vec<PartyInput>,
    pub payout_spk: ScriptBuf,
    pub change_spk: ScriptBuf,
    pub wallet: Option<Arc<DlcDevKitWallet>>,
}

/// How to build a party.
pub struct PartySpec {
    pub role: Party,
    /// Distinguishes this party's key material from the other's.
    pub seed_byte: u8,
    pub input_source: InputSource,
    pub funding_key_source: FundingKeySource,
    /// One on-chain UTXO is funded per entry, with the matching serial id.
    pub utxos: Vec<(Amount, u64)>,
}

impl PartySpec {
    /// A dual-funded party with a single UTXO.
    pub fn new(role: Party, seed_byte: u8, serial_id: u64) -> Self {
        Self {
            role,
            seed_byte,
            input_source: InputSource::Xpriv,
            funding_key_source: FundingKeySource::Xprv,
            utxos: vec![(UTXO_VALUE, serial_id)],
        }
    }

    /// A party that contributes no funding inputs (the accepting side of a
    /// single-funded contract, or the accepting side of a splice).
    pub fn unfunded(role: Party, seed_byte: u8) -> Self {
        Self {
            role,
            seed_byte,
            input_source: InputSource::Xpriv,
            funding_key_source: FundingKeySource::Xprv,
            utxos: vec![],
        }
    }

    pub fn with_input_source(mut self, input_source: InputSource) -> Self {
        self.input_source = input_source;
        self
    }

    pub fn with_funding_key_source(mut self, funding_key_source: FundingKeySource) -> Self {
        self.funding_key_source = funding_key_source;
        self
    }

    pub fn with_utxos(mut self, utxos: Vec<(Amount, u64)>) -> Self {
        self.utxos = utxos;
        self
    }
}

impl TestParty {
    /// Builds a party: derives its keys, funds its UTXOs on-chain, and (for
    /// [`InputSource::DdkWallet`]) stands up a real wallet that owns them.
    pub async fn new(ctx: &ChainContext, spec: PartySpec, temporary_contract_id: [u8; 32]) -> Self {
        let secp = Secp256k1::new();
        let xpriv = Xpriv::new_master(NETWORK, &[spec.seed_byte; 64]).unwrap();
        let (contract_keys, funding_secret_key) =
            funding_key(&spec, &xpriv, temporary_contract_id, &secp);

        let wallet = if spec.input_source == InputSource::DdkWallet {
            Some(Arc::new(
                DlcDevKitWallet::new(
                    &[spec.seed_byte.wrapping_add(50); 64],
                    ctx.esplora.clone(),
                    NETWORK,
                    Arc::new(MemoryStorage::new()),
                    None,
                    ctx.logger.clone(),
                )
                .await
                .expect("could not create the wallet"),
            ))
        } else {
            None
        };

        let mut inputs = Vec::with_capacity(spec.utxos.len());
        for (derivation_index, (value, serial_id)) in spec.utxos.iter().enumerate() {
            let derivation_index = derivation_index as u32;
            let script_pubkey = match &wallet {
                // The wallet must own the UTXO it is later asked to sign.
                Some(wallet) => wallet
                    .new_external_address()
                    .await
                    .unwrap()
                    .address
                    .script_pubkey(),
                None => p2wpkh_script(&secp, &xpriv, &input_path(derivation_index)),
            };
            let (previous_transaction, vout) = ctx.fund_script(&script_pubkey, *value).await;
            inputs.push(PartyInput {
                funding_input: funding_input(
                    &previous_transaction,
                    vout,
                    Some(*serial_id),
                    u32::MAX,
                    P2WPKH_MAX_WITNESS_LEN,
                    ScriptBuf::new(),
                )
                .unwrap(),
                derivation_index,
            });
        }
        if let Some(wallet) = &wallet {
            wallet.sync().await.expect("could not sync the wallet");
        }

        Self {
            role: spec.role,
            input_source: spec.input_source,
            funding_key_source: spec.funding_key_source,
            xpriv,
            contract_keys,
            funding_secret_key,
            inputs,
            payout_spk: p2wpkh_script(&secp, &xpriv, &input_path(PAYOUT_INDEX)),
            change_spk: p2wpkh_script(&secp, &xpriv, &input_path(CHANGE_INDEX)),
            wallet,
        }
    }

    pub fn funding_pubkey(&self) -> PublicKey {
        self.funding_secret_key.public_key(&Secp256k1::new())
    }

    pub fn funding_inputs(&self) -> Vec<FundingInput> {
        self.inputs
            .iter()
            .map(|input| input.funding_input.clone())
            .collect()
    }

    pub fn party_params(&self, extra_inputs: Vec<FundingInput>) -> PartyParams {
        let mut funding_inputs = extra_inputs;
        funding_inputs.extend(self.funding_inputs());
        PartyParams {
            funding_pubkey: self.funding_pubkey(),
            funding_inputs,
            payout_spk: self.payout_spk.clone(),
            payout_serial_id: None,
            change_spk: self.change_spk.clone(),
            change_serial_id: None,
        }
    }

    /// Recovers this party's funding key for a *previous* contract, so it can
    /// sign that contract's 2-of-2 output when splicing.
    ///
    /// Providers recompute the key from the previous temporary contract id;
    /// a raw key is simply the one the party already holds.
    pub fn dlc_input_signing_key(
        &self,
        prior_temporary_contract_id: [u8; 32],
        input_serial_id: u64,
    ) -> DlcInputSigningKey {
        match &self.contract_keys {
            Some(keys) => keys
                .dlc_input_signing_key(prior_temporary_contract_id, input_serial_id)
                .unwrap(),
            None => DlcInputSigningKey {
                input_serial_id,
                prior_funding_secret_key: self.funding_secret_key,
            },
        }
    }

    /// Signs and finalizes this party's funding inputs in the PSBT, through
    /// whichever signer the party was built with.
    pub async fn sign_funding_psbt(
        &self,
        offer: &OfferDlc,
        accept: &AcceptDlc,
        psbt: &mut Psbt,
    ) -> Result<(), ContractError> {
        if self.inputs.is_empty() {
            return Ok(());
        }
        match self.input_source {
            InputSource::Xpriv => {
                let derivations: Vec<InputDerivation> = self
                    .inputs
                    .iter()
                    .map(|input| InputDerivation {
                        input_serial_id: input.funding_input.input_serial_id,
                        derivation_path: input_path(input.derivation_index),
                    })
                    .collect();
                signing::sign_funding_psbt_with_xpriv(
                    offer,
                    accept,
                    psbt,
                    &self.xpriv,
                    &derivations,
                )
            }
            InputSource::Descriptor => {
                let descriptor = format!("wpkh({}/{BIP84_ACCOUNT}/0/*)", self.xpriv);
                let inputs: Vec<DescriptorInput> = self
                    .inputs
                    .iter()
                    .map(|input| DescriptorInput {
                        input_serial_id: input.funding_input.input_serial_id,
                        derivation_index: input.derivation_index,
                    })
                    .collect();
                signing::sign_funding_psbt_with_descriptor(
                    offer,
                    accept,
                    psbt,
                    &descriptor,
                    &inputs,
                )
            }
            InputSource::DdkWallet => {
                let wallet = self.wallet.as_ref().expect("wallet party has a wallet");
                signing::sign_funding_psbt_with_wallet(
                    offer,
                    accept,
                    psbt,
                    wallet.as_ref(),
                    self.role,
                )
                .await
            }
            InputSource::ExternalSigner => {
                let paths: Vec<DerivationPath> = self
                    .inputs
                    .iter()
                    .map(|input| input_path(input.derivation_index))
                    .collect();
                *psbt = external_signer(psbt, &self.xpriv, &paths);
                Ok(())
            }
        }
    }
}

fn funding_key(
    spec: &PartySpec,
    xpriv: &Xpriv,
    temporary_contract_id: [u8; 32],
    secp: &Secp256k1<All>,
) -> (Option<ContractKeyProvider>, SecretKey) {
    let provider = match spec.funding_key_source {
        FundingKeySource::RawSecretKey => {
            let secret_key = SecretKey::from_slice(&[spec.seed_byte; 32]).unwrap();
            let _ = secp;
            return (None, secret_key);
        }
        FundingKeySource::Xprv => ContractKeyProvider::from_xprv(*xpriv),
        FundingKeySource::Seed => {
            ContractKeyProvider::from_seed(&[spec.seed_byte.wrapping_add(7); 64], NETWORK).unwrap()
        }
        FundingKeySource::Mnemonic => {
            let mnemonic = match spec.role {
                Party::Offer => OFFER_MNEMONIC,
                Party::Accept => ACCEPT_MNEMONIC,
            };
            ContractKeyProvider::from_mnemonic(mnemonic, None, NETWORK).unwrap()
        }
        FundingKeySource::Descriptor => {
            ContractKeyProvider::from_descriptor(&format!("wpkh({xpriv}/{BIP84_ACCOUNT}/0/*)"))
                .unwrap()
        }
    };
    let funding_secret_key = provider.funding_secret_key(temporary_contract_id).unwrap();
    (Some(provider), funding_secret_key)
}

fn input_path(index: u32) -> DerivationPath {
    DerivationPath::from_str(&format!("{BIP84_ACCOUNT}/0/{index}")).unwrap()
}

pub fn p2wpkh_script(secp: &Secp256k1<All>, xpriv: &Xpriv, path: &DerivationPath) -> ScriptBuf {
    let public_key = xpriv
        .derive_priv(secp, path)
        .unwrap()
        .to_priv()
        .public_key(secp);
    ScriptBuf::new_p2wpkh(&public_key.wpubkey_hash().unwrap())
}

/// Simulates a wallet outside DDK: the PSBT is serialized, signed and finalized
/// with nothing but rust-bitcoin, and returned.
///
/// Only inputs whose script is controlled by one of `paths` are touched; the
/// counterparty's inputs are left untouched.
fn external_signer(psbt: &Psbt, xpriv: &Xpriv, paths: &[DerivationPath]) -> Psbt {
    let secp = Secp256k1::new();
    let mut external = Psbt::deserialize(&psbt.serialize()).unwrap();
    let fingerprint = xpriv.fingerprint(&secp);
    for path in paths {
        let private_key = xpriv.derive_priv(&secp, path).unwrap().to_priv();
        let public_key = private_key.public_key(&secp);
        let owned_script = ScriptBuf::new_p2wpkh(&public_key.wpubkey_hash().unwrap());
        for index in 0..external.inputs.len() {
            let owns_input = external.inputs[index]
                .witness_utxo
                .as_ref()
                .map(|utxo| utxo.script_pubkey == owned_script)
                .unwrap_or(false);
            if owns_input {
                external.inputs[index]
                    .bip32_derivation
                    .insert(public_key.inner, (fingerprint, path.clone()));
            }
        }
    }
    external.sign(xpriv, &secp).unwrap();
    for index in 0..external.inputs.len() {
        let Some((public_key, signature)) = external.inputs[index]
            .partial_sigs
            .iter()
            .map(|(pk, sig)| (*pk, *sig))
            .next()
        else {
            continue;
        };
        external.inputs[index].final_script_witness = Some(Witness::from_slice(&[
            signature.to_vec(),
            public_key.to_bytes(),
        ]));
        external.inputs[index].partial_sigs.clear();
    }
    Psbt::deserialize(&external.serialize()).unwrap()
}

/// The splice half of a [`ContractSetup`]: a previous contract's funding output
/// plus each party's key for it.
pub struct SpliceSetup {
    pub funding_input: FundingInput,
    pub offer_key: DlcInputSigningKey,
    pub accept_key: DlcInputSigningKey,
}

/// Everything needed to drive one contract from offer to a confirmed funding
/// transaction.
pub struct ContractSetup {
    pub contract_info: ContractInfo,
    pub offer_collateral: Amount,
    pub temporary_contract_id: [u8; 32],
    pub offerer: TestParty,
    pub accepter: TestParty,
    pub splice: Option<SpliceSetup>,
}

impl ContractSetup {
    pub fn new(
        contract_info: ContractInfo,
        offer_collateral: Amount,
        temporary_contract_id: [u8; 32],
        offerer: TestParty,
        accepter: TestParty,
    ) -> Self {
        Self {
            contract_info,
            offer_collateral,
            temporary_contract_id,
            offerer,
            accepter,
            splice: None,
        }
    }

    pub fn with_splice(mut self, splice: SpliceSetup) -> Self {
        self.splice = Some(splice);
        self
    }
}

/// A contract whose funding transaction is confirmed on chain.
///
/// The three wire messages are the only contract state that exists; everything
/// else here is derived from them.
pub struct FundedContract {
    pub offer: OfferDlc,
    pub accept: AcceptDlc,
    pub sign: SignDlc,
    pub funding_transaction: Transaction,
    pub transactions: DlcTransactions,
    pub offerer: TestParty,
    pub accepter: TestParty,
    pub temporary_contract_id: [u8; 32],
}

impl FundedContract {
    pub fn fund_outpoint(&self) -> OutPoint {
        OutPoint {
            txid: self.funding_transaction.compute_txid(),
            vout: self.transactions.get_fund_output_index() as u32,
        }
    }

    pub fn fund_value(&self) -> Amount {
        self.transactions.get_fund_output().value
    }

    pub fn party(&self, party: Party) -> &TestParty {
        match party {
            Party::Offer => &self.offerer,
            Party::Accept => &self.accepter,
        }
    }

    /// Builds the splice input that spends this contract's funding output,
    /// with each party recovering its own key for it.
    pub fn splice_setup(&self, input_serial_id: u64) -> SpliceSetup {
        self.splice_setup_by(Party::Offer, input_serial_id)
    }

    /// A splice of this contract offered by the side `splicer` names.
    ///
    /// The keys travel with the roles: `local_fund_pubkey` is the splicing
    /// party's key in this contract, so the party that recovers it is the one
    /// that offers the replacement.
    pub fn splice_setup_by(&self, splicer: Party, input_serial_id: u64) -> SpliceSetup {
        match splicer {
            Party::Offer => splice_from(
                self,
                splicer,
                &self.offerer,
                &self.accepter,
                input_serial_id,
            ),
            Party::Accept => splice_from(
                self,
                splicer,
                &self.accepter,
                &self.offerer,
                input_serial_id,
            ),
        }
    }
}

/// Builds a splice input over `previous`'s funding output, with `offerer` and
/// `accepter` each recovering their previous-contract funding key from their
/// own key source.
///
/// `splicer` names the side of `previous` that offers the replacement contract.
/// It decides the order of the two keys in the [`DlcInput`], so `offerer` must
/// be the party that held `previous`'s `splicer` side, and `accepter` the other
/// one. Getting that pair the wrong way round is what the API's own key checks
/// catch.
///
/// The parties passed here are the ones signing the *new* contract; they may be
/// freshly constructed, which is the point — nothing about the previous
/// contract's keys was carried over, only its temporary id and wire messages.
pub fn splice_from(
    previous: &FundedContract,
    splicer: Party,
    offerer: &TestParty,
    accepter: &TestParty,
    input_serial_id: u64,
) -> SpliceSetup {
    SpliceSetup {
        funding_input: create_dlc_splice_input(
            &previous.offer,
            &previous.accept,
            splicer,
            Some(input_serial_id),
            DLC_INPUT_MAX_WITNESS_LEN,
        )
        .unwrap(),
        offer_key: offerer.dlc_input_signing_key(previous.temporary_contract_id, input_serial_id),
        accept_key: accepter.dlc_input_signing_key(previous.temporary_contract_id, input_serial_id),
    }
}

/// Runs a contract from `create_offer` through to a confirmed funding
/// transaction, using only the stateless API and the wire messages.
pub async fn fund_contract(ctx: &ChainContext, setup: ContractSetup) -> FundedContract {
    let ContractSetup {
        contract_info,
        offer_collateral,
        temporary_contract_id,
        offerer,
        accepter,
        splice,
    } = setup;

    let splice_inputs = splice
        .as_ref()
        .map(|splice| vec![splice.funding_input.clone()])
        .unwrap_or_default();
    let offer_dlc_keys: Vec<DlcInputSigningKey> = splice
        .as_ref()
        .map(|splice| vec![splice.offer_key.clone()])
        .unwrap_or_default();
    let accept_dlc_keys: Vec<DlcInputSigningKey> = splice
        .as_ref()
        .map(|splice| vec![splice.accept_key.clone()])
        .unwrap_or_default();

    let offer = create_offer(CreateOfferParams {
        chain_hash: chain_hash_from_network(NETWORK),
        temporary_contract_id: Some(temporary_contract_id),
        contract_info,
        offer_collateral,
        party: offerer.party_params(splice_inputs),
        fund_output_serial_id: None,
        fee_rate_per_vb: FEE_RATE_PER_VB,
        cet_locktime: EVENT_MATURITY,
        refund_locktime: EVENT_MATURITY + REFUND_DELAY,
        contract_flags: 0,
    })
    .expect("could not create the offer");

    let accept_result = accept_offer(
        &offer,
        AcceptOfferParams {
            party: accepter.party_params(vec![]),
            min_timeout_interval: MIN_TIMEOUT_INTERVAL,
            max_timeout_interval: MAX_TIMEOUT_INTERVAL,
        },
        &accepter.funding_secret_key,
    )
    .expect("could not accept the offer");
    let accept = accept_result.accept;

    // The offering party signs its own inputs, then the accept message.
    let mut offer_psbt = create_funding_psbt(&offer, &accept).unwrap();
    offerer
        .sign_funding_psbt(&offer, &accept, &mut offer_psbt)
        .await
        .expect("the offering party could not sign the funding PSBT");
    let sign = sign_accept_spliced(
        &offer,
        &accept,
        &offerer.funding_secret_key,
        &offer_psbt,
        &offer_dlc_keys,
    )
    .expect("could not create the sign message")
    .sign;

    // The accepting party signs its own inputs and completes the transaction.
    let mut accept_psbt = create_funding_psbt(&offer, &accept).unwrap();
    accepter
        .sign_funding_psbt(&offer, &accept, &mut accept_psbt)
        .await
        .expect("the accepting party could not sign the funding PSBT");
    let funding_transaction =
        finalize_sign_spliced(&offer, &accept, &sign, &accept_psbt, &accept_dlc_keys)
            .expect("could not finalize the funding transaction");

    let transactions = create_dlc_transactions(&offer, &accept).unwrap();
    assert_eq!(
        funding_transaction.compute_txid(),
        transactions.fund.compute_txid(),
        "the completed funding transaction is not the one rebuilt from the messages"
    );
    assert_eq!(
        funding_transaction.input.len(),
        offer.funding_inputs.len() + accept.funding_inputs.len()
    );
    assert!(
        funding_transaction
            .input
            .iter()
            .all(|input| !input.witness.is_empty()),
        "every funding input must carry a witness"
    );

    ctx.broadcast_and_confirm(&funding_transaction, 6).await;

    FundedContract {
        offer,
        accept,
        sign,
        funding_transaction,
        transactions,
        offerer,
        accepter,
        temporary_contract_id,
    }
}

/// Settles the contract by broadcasting the CET for `attestations`.
pub async fn close_with_cet(
    ctx: &ChainContext,
    contract: &FundedContract,
    closer: Party,
    attestations: &[(usize, OracleAttestation)],
) -> Transaction {
    let cet = sign_cet(
        &contract.offer,
        &contract.accept,
        &contract.sign,
        &contract.party(closer).funding_secret_key,
        attestations,
    )
    .expect("could not sign the CET");
    assert!(
        contract
            .transactions
            .cets
            .iter()
            .any(|candidate| candidate.compute_txid() == cet.compute_txid()),
        "the signed CET is not one of the CETs rebuilt from the messages"
    );
    assert_eq!(cet.input.len(), 1);
    assert_eq!(cet.input[0].previous_output, contract.fund_outpoint());
    for output in &cet.output {
        assert!(
            output.script_pubkey == contract.offer.payout_spk
                || output.script_pubkey == contract.accept.payout_spk,
            "a CET output does not pay either party"
        );
    }
    assert!(
        cet.output.iter().map(|output| output.value).sum::<Amount>() < contract.fund_value(),
        "the CET must pay a fee out of the funding output"
    );

    ctx.broadcast_and_confirm(&cet, 1).await;
    ctx.assert_spent_by(contract.fund_outpoint(), &cet).await;
    cet
}

/// Settles the contract by broadcasting the refund transaction.
///
/// Both parties signed the refund during the offer/accept exchange; `closer`
/// contributes the second half of the 2-of-2 here.
pub async fn close_with_refund(
    ctx: &ChainContext,
    contract: &FundedContract,
    closer: Party,
) -> Transaction {
    let refund = sign_refund(
        &contract.offer,
        &contract.accept,
        &contract.sign,
        &contract.party(closer).funding_secret_key,
    )
    .expect("could not sign the refund transaction");

    assert_eq!(
        refund.compute_txid(),
        contract.transactions.refund.compute_txid(),
        "the refund is not the one rebuilt from the messages"
    );
    assert_eq!(refund.input[0].previous_output, contract.fund_outpoint());
    ctx.broadcast_and_confirm(&refund, 1).await;
    ctx.assert_spent_by(contract.fund_outpoint(), &refund).await;
    refund
}
