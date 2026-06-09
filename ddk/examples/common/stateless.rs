// Shared scaffolding for the stateless contract examples: deterministic
// keys, a dummy funding UTXO per party, and a simple enum contract. Nothing
// here touches a chain, storage backend, or contract manager.

use bitcoin::absolute::LockTime;
use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::transaction::Version;
use bitcoin::{
    Amount, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness,
};
use ddk::contract::{
    chain_hash_from_network, funding_input, CreateOfferParams, InputDerivation, PartyParams,
};
use ddk_dlc::secp256k1_zkp::{All, Keypair, PublicKey, Secp256k1, SecretKey, XOnlyPublicKey};
use ddk_messages::contract_msgs::{
    ContractDescriptor, ContractInfo, ContractInfoInner, ContractOutcome,
    EnumeratedContractDescriptor, SingleContractInfo,
};
use ddk_messages::oracle_msgs::{
    tagged_announcement_msg, EnumEventDescriptor, EventDescriptor, OracleAnnouncement,
    OracleEvent, OracleInfo, SingleOracleInfo,
};
use ddk_messages::FundingInput;
use std::str::FromStr;

pub const TOTAL_COLLATERAL: Amount = Amount::from_sat(100_000);

/// One side of a contract: a DLC funding key plus a BIP84 wallet key
/// controlling a single funding UTXO.
pub struct PartySetup {
    pub funding_secret_key: SecretKey,
    pub xpriv: Xpriv,
    pub derivation_path: DerivationPath,
    pub funding_input: FundingInput,
}

impl PartySetup {
    pub fn new(secp: &Secp256k1<All>, seed_byte: u8, network: Network, utxo_value: Amount) -> Self {
        let funding_secret_key = SecretKey::from_slice(&[seed_byte; 32]).unwrap();
        let xpriv = Xpriv::new_master(network, &[seed_byte.wrapping_add(100); 64]).unwrap();
        let coin_type = if network == Network::Bitcoin { 0 } else { 1 };
        let derivation_path =
            DerivationPath::from_str(&format!("84h/{coin_type}h/0h/0/0")).unwrap();
        let script_pubkey = p2wpkh_script(secp, &xpriv, &derivation_path);
        let funding_input = funding_input(
            &previous_transaction(utxo_value, script_pubkey),
            0,
            Some(seed_byte as u64),
            u32::MAX,
            108,
            ScriptBuf::new(),
        )
        .unwrap();
        Self {
            funding_secret_key,
            xpriv,
            derivation_path,
            funding_input,
        }
    }

    pub fn funding_pubkey(&self, secp: &Secp256k1<All>) -> PublicKey {
        self.funding_secret_key.public_key(secp)
    }

    pub fn payout_script(&self, secp: &Secp256k1<All>) -> ScriptBuf {
        p2wpkh_script(secp, &self.xpriv, &self.derivation_path)
    }

    pub fn party_params(&self, secp: &Secp256k1<All>) -> PartyParams {
        self.party_params_with_inputs(secp, vec![self.funding_input.clone()])
    }

    pub fn party_params_with_inputs(
        &self,
        secp: &Secp256k1<All>,
        funding_inputs: Vec<FundingInput>,
    ) -> PartyParams {
        PartyParams {
            funding_pubkey: self.funding_pubkey(secp),
            funding_inputs,
            payout_spk: self.payout_script(secp),
            payout_serial_id: None,
            change_spk: self.payout_script(secp),
            change_serial_id: None,
        }
    }

    pub fn derivations(&self) -> Vec<InputDerivation> {
        vec![InputDerivation {
            input_serial_id: self.funding_input.input_serial_id,
            derivation_path: self.derivation_path.clone(),
        }]
    }
}

pub fn p2wpkh_script(secp: &Secp256k1<All>, xpriv: &Xpriv, path: &DerivationPath) -> ScriptBuf {
    let public_key = xpriv
        .derive_priv(secp, path)
        .unwrap()
        .to_priv()
        .public_key(secp);
    ScriptBuf::new_p2wpkh(&public_key.wpubkey_hash().unwrap())
}

/// A fake confirmed transaction paying `value` to `script_pubkey`.
pub fn previous_transaction(value: Amount, script_pubkey: ScriptBuf) -> Transaction {
    Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value,
            script_pubkey,
        }],
    }
}

/// A two-outcome enum contract with a locally signed oracle announcement.
pub fn enum_contract_info(total_collateral: Amount) -> ContractInfo {
    let secp = Secp256k1::new();
    let oracle_key = Keypair::from_secret_key(&secp, &SecretKey::from_slice(&[88; 32]).unwrap());
    let nonce_key = Keypair::from_secret_key(&secp, &SecretKey::from_slice(&[90; 32]).unwrap());
    let oracle_event = OracleEvent {
        oracle_nonces: vec![XOnlyPublicKey::from_keypair(&nonce_key).0],
        event_maturity_epoch: 750,
        event_descriptor: EventDescriptor::EnumEvent(EnumEventDescriptor {
            outcomes: vec!["up".to_string(), "down".to_string()],
        }),
        event_id: "stateless-example".to_string(),
    };
    let announcement = OracleAnnouncement {
        announcement_signature: secp
            .sign_schnorr(&tagged_announcement_msg(&oracle_event), &oracle_key),
        oracle_public_key: XOnlyPublicKey::from_keypair(&oracle_key).0,
        oracle_event,
    };
    ContractInfo::SingleContractInfo(SingleContractInfo {
        total_collateral,
        contract_info: ContractInfoInner {
            contract_descriptor: ContractDescriptor::EnumeratedContractDescriptor(
                EnumeratedContractDescriptor {
                    payouts: vec![
                        ContractOutcome {
                            outcome: "up".to_string(),
                            offer_payout: total_collateral,
                        },
                        ContractOutcome {
                            outcome: "down".to_string(),
                            offer_payout: Amount::ZERO,
                        },
                    ],
                },
            ),
            oracle_info: OracleInfo::Single(SingleOracleInfo {
                oracle_announcement: announcement,
            }),
        },
    })
}

pub fn offer_params(
    secp: &Secp256k1<All>,
    offerer: &PartySetup,
    offer_collateral: Amount,
    network: Network,
) -> CreateOfferParams {
    offer_params_with_party(
        offerer.party_params(secp),
        offer_collateral,
        network,
    )
}

pub fn offer_params_with_party(
    party: PartyParams,
    offer_collateral: Amount,
    network: Network,
) -> CreateOfferParams {
    CreateOfferParams {
        chain_hash: chain_hash_from_network(network),
        temporary_contract_id: None,
        contract_info: enum_contract_info(TOTAL_COLLATERAL),
        offer_collateral,
        party,
        fund_output_serial_id: None,
        fee_rate_per_vb: 2,
        cet_locktime: 500,
        refund_locktime: 1_000,
        contract_flags: 0,
    }
}
