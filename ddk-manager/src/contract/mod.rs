//! Module containing structures and functions related to contracts.

use crate::error::Error;
use crate::ContractId;
use bitcoin::{Amount, SignedAmount, Transaction, Txid};
use dlc_messages::{
    oracle_msgs::{EventDescriptor, OracleAnnouncement, OracleAttestation},
    AcceptDlc, SignDlc,
};
use dlc_trie::multi_oracle_trie::MultiOracleTrie;
use dlc_trie::multi_oracle_trie_with_diff::MultiOracleTrieWithDiff;
use secp256k1_zkp::PublicKey;
#[cfg(feature = "use-serde")]
use serde::{Deserialize, Serialize};
use signed_contract::SignedContract;
use std::fmt::Write;

use self::utils::unordered_equal;

pub mod accepted_contract;
pub mod contract_info;
pub mod contract_input;
pub mod enum_descriptor;
pub mod numerical_descriptor;
pub mod offered_contract;
pub mod ser;
pub mod signed_contract;
pub(crate) mod utils;

#[derive(Clone)]
/// Enum representing the possible states of a DLC.
pub enum Contract {
    /// Initial state where a contract is being proposed.
    Offered(offered_contract::OfferedContract),
    /// A contract that was accepted.
    Accepted(accepted_contract::AcceptedContract),
    /// A contract for which signatures have been produced.
    Signed(signed_contract::SignedContract),
    /// A contract whose funding transaction was included in the blockchain with sufficient confirmations.
    Confirmed(signed_contract::SignedContract),
    /// A contract for which a CET was broadcasted, but not fully confirmed to the blockchain.
    PreClosed(PreClosedContract),
    /// A contract for which a CET was fully confirmed to blockchain
    Closed(ClosedContract),
    /// A contract whose refund transaction was broadcast.
    Refunded(signed_contract::SignedContract),
    /// A contract that failed when verifying information from an accept message.
    FailedAccept(FailedAcceptContract),
    /// A contract that failed when verifying information from a sign message.
    FailedSign(FailedSignContract),
    /// A contract that was rejected by the party to whom it was offered.
    Rejected(offered_contract::OfferedContract),
}

impl std::fmt::Debug for Contract {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match self {
            Contract::Offered(_) => "offered",
            Contract::Accepted(_) => "accepted",
            Contract::Signed(_) => "signed",
            Contract::Confirmed(_) => "confirmed",
            Contract::PreClosed(_) => "pre-closed",
            Contract::Closed(_) => "closed",
            Contract::Refunded(_) => "refunded",
            Contract::FailedAccept(_) => "failed accept",
            Contract::FailedSign(_) => "failed sign",
            Contract::Rejected(_) => "rejected",
        };
        f.debug_struct("Contract").field("state", &state).finish()
    }
}

impl Contract {
    /// Get the id of a contract. Returns the temporary contract id for offered
    /// and failed accept contracts.
    pub fn get_id(&self) -> ContractId {
        match self {
            Contract::Offered(o) | Contract::Rejected(o) => o.id,
            Contract::Accepted(o) => o.get_contract_id(),
            Contract::Signed(o) | Contract::Confirmed(o) | Contract::Refunded(o) => {
                o.accepted_contract.get_contract_id()
            }
            Contract::FailedAccept(c) => c.offered_contract.id,
            Contract::FailedSign(c) => c.accepted_contract.get_contract_id(),
            Contract::PreClosed(c) => c.signed_contract.accepted_contract.get_contract_id(),
            Contract::Closed(c) => c.contract_id,
        }
    }

    /// Get the string representation of the contract id.
    pub fn get_id_string(&self) -> String {
        let mut string_id = String::with_capacity(32 * 2 + 2);
        string_id.push_str("0x");
        let id = self.get_id();
        for i in &id {
            write!(string_id, "{i:02x}").unwrap();
        }

        string_id
    }

    /// Returns the temporary contract id of a contract.
    pub fn get_temporary_id(&self) -> ContractId {
        match self {
            Contract::Offered(o) | Contract::Rejected(o) => o.id,
            Contract::Accepted(o) => o.offered_contract.id,
            Contract::Signed(o) | Contract::Confirmed(o) | Contract::Refunded(o) => {
                o.accepted_contract.offered_contract.id
            }
            Contract::FailedAccept(c) => c.offered_contract.id,
            Contract::FailedSign(c) => c.accepted_contract.offered_contract.id,
            Contract::PreClosed(c) => c.signed_contract.accepted_contract.offered_contract.id,
            Contract::Closed(c) => c.temporary_contract_id,
        }
    }

    /// Returns the public key of the counter party's node.
    pub fn get_counter_party_id(&self) -> PublicKey {
        match self {
            Contract::Offered(o) | Contract::Rejected(o) => o.counter_party,
            Contract::Accepted(a) => a.offered_contract.counter_party,
            Contract::Signed(s) | Contract::Confirmed(s) | Contract::Refunded(s) => {
                s.accepted_contract.offered_contract.counter_party
            }
            Contract::PreClosed(c) => {
                c.signed_contract
                    .accepted_contract
                    .offered_contract
                    .counter_party
            }
            Contract::Closed(c) => c.counter_party_id,
            Contract::FailedAccept(f) => f.offered_contract.counter_party,
            Contract::FailedSign(f) => f.accepted_contract.offered_contract.counter_party,
        }
    }

    /// Checks if the contract is the offer party.
    pub fn is_offer_party(&self) -> bool {
        match self {
            Contract::Offered(o) | Contract::Rejected(o) => o.is_offer_party,
            Contract::Accepted(a) => a.offered_contract.is_offer_party,
            Contract::Signed(s) | Contract::Confirmed(s) | Contract::Refunded(s) => {
                s.accepted_contract.offered_contract.is_offer_party
            }
            Contract::FailedAccept(f) => f.offered_contract.is_offer_party,
            Contract::FailedSign(f) => f.accepted_contract.offered_contract.is_offer_party,
            Contract::PreClosed(c) => {
                c.signed_contract
                    .accepted_contract
                    .offered_contract
                    .is_offer_party
            }
            Contract::Closed(_) => false,
        }
    }

    /// Get the collateral for a contract.
    pub fn get_collateral(
        &self,
    ) -> (
        Amount, /* offer collateral */
        Amount, /* accept collateral */
        Amount, /* total collateral */
    ) {
        // TODO: We should assert that the offer + accept collateral is equal to the total collateral
        match self {
            Contract::Offered(o) => (
                o.offer_params.collateral,
                o.total_collateral - o.offer_params.collateral,
                o.total_collateral,
            ),
            Contract::Accepted(a) => (
                a.offered_contract.offer_params.collateral,
                a.accept_params.collateral,
                a.offered_contract.total_collateral,
            ),
            Contract::Signed(s) | Contract::Confirmed(s) | Contract::Refunded(s) => (
                s.accepted_contract.offered_contract.offer_params.collateral,
                s.accepted_contract.accept_params.collateral,
                s.accepted_contract.offered_contract.total_collateral,
            ),
            Contract::FailedAccept(f) => (
                f.offered_contract.offer_params.collateral,
                Amount::ZERO,
                f.offered_contract.total_collateral,
            ),
            Contract::FailedSign(f) => (
                f.accepted_contract.offered_contract.offer_params.collateral,
                f.accepted_contract.accept_params.collateral,
                f.accepted_contract.offered_contract.total_collateral,
            ),
            Contract::PreClosed(p) => (
                p.signed_contract
                    .accepted_contract
                    .offered_contract
                    .offer_params
                    .collateral,
                p.signed_contract.accepted_contract.accept_params.collateral,
                p.signed_contract
                    .accepted_contract
                    .offered_contract
                    .total_collateral,
            ),
            Contract::Closed(_) => (Amount::ZERO, Amount::ZERO, Amount::ZERO),
            Contract::Rejected(_) => (Amount::ZERO, Amount::ZERO, Amount::ZERO),
        }
    }

    /// Get the CET locktime for a contract.
    pub fn get_cet_locktime(&self) -> u32 {
        match self {
            Contract::Offered(o) => o.cet_locktime,
            Contract::Accepted(a) => a.offered_contract.cet_locktime,
            Contract::Signed(s) => s.accepted_contract.offered_contract.cet_locktime,
            Contract::Confirmed(c) => c.accepted_contract.offered_contract.cet_locktime,
            Contract::PreClosed(p) => {
                p.signed_contract
                    .accepted_contract
                    .offered_contract
                    .cet_locktime
            }
            Contract::Closed(c) => c.signed_cet.as_ref().unwrap().lock_time.to_consensus_u32(),
            Contract::Refunded(r) => r.accepted_contract.offered_contract.cet_locktime,
            Contract::FailedAccept(f) => f.offered_contract.cet_locktime,
            Contract::FailedSign(f) => f.accepted_contract.offered_contract.cet_locktime,
            Contract::Rejected(_) => 0,
        }
    }

    /// Get the refund locktime for a contract.
    pub fn get_refund_locktime(&self) -> u32 {
        match self {
            Contract::Offered(o) => o.refund_locktime,
            Contract::Accepted(a) => a.offered_contract.refund_locktime,
            Contract::Signed(s) => s.accepted_contract.offered_contract.refund_locktime,
            Contract::Confirmed(c) => c.accepted_contract.offered_contract.refund_locktime,
            Contract::PreClosed(p) => {
                p.signed_contract
                    .accepted_contract
                    .offered_contract
                    .refund_locktime
            }
            Contract::Closed(c) => c.signed_cet.as_ref().unwrap().lock_time.to_consensus_u32(),
            Contract::Refunded(r) => r.accepted_contract.offered_contract.refund_locktime,
            Contract::FailedAccept(f) => f.offered_contract.refund_locktime,
            Contract::FailedSign(f) => f.accepted_contract.offered_contract.refund_locktime,
            Contract::Rejected(_) => 0,
        }
    }

    /// Get the profit and loss for a contract.
    pub fn get_pnl(&self) -> SignedAmount {
        match self {
            Contract::Offered(_) => SignedAmount::ZERO,
            Contract::Accepted(_) => SignedAmount::ZERO,
            Contract::Signed(_) => SignedAmount::ZERO,
            Contract::Confirmed(_) => SignedAmount::ZERO,
            Contract::PreClosed(p) => p
                .signed_contract
                .accepted_contract
                .compute_pnl(&p.signed_cet),
            Contract::Closed(c) => c.pnl,
            Contract::Refunded(_) => SignedAmount::ZERO,
            Contract::FailedAccept(_) => SignedAmount::ZERO,
            Contract::FailedSign(_) => SignedAmount::ZERO,
            Contract::Rejected(_) => SignedAmount::ZERO,
        }
    }

    /// Get the funding txid for a contract.
    pub fn get_funding_txid(&self) -> Option<Txid> {
        match self {
            Contract::Offered(_) => None,
            Contract::Accepted(a) => Some(a.dlc_transactions.fund.compute_txid()),
            Contract::Signed(s) => Some(s.accepted_contract.dlc_transactions.fund.compute_txid()),
            Contract::Confirmed(c) => {
                Some(c.accepted_contract.dlc_transactions.fund.compute_txid())
            }
            Contract::PreClosed(p) => Some(
                p.signed_contract
                    .accepted_contract
                    .dlc_transactions
                    .fund
                    .compute_txid(),
            ),
            Contract::Closed(c) => Some(c.funding_txid),
            Contract::Refunded(r) => Some(r.accepted_contract.dlc_transactions.fund.compute_txid()),
            Contract::FailedAccept(_) => None,
            Contract::FailedSign(_) => None,
            Contract::Rejected(_) => None,
        }
    }

    /// Get the oracle announcement for a contract.
    pub fn get_oracle_announcement(&self) -> Option<OracleAnnouncement> {
        match self {
            Contract::Offered(o) => Some(o.contract_info[0].oracle_announcements[0].clone()),
            Contract::Accepted(a) => {
                Some(a.offered_contract.contract_info[0].oracle_announcements[0].clone())
            }
            Contract::Signed(s) => Some(
                s.accepted_contract.offered_contract.contract_info[0].oracle_announcements[0]
                    .clone(),
            ),
            Contract::Confirmed(c) => Some(
                c.accepted_contract.offered_contract.contract_info[0].oracle_announcements[0]
                    .clone(),
            ),
            Contract::PreClosed(p) => Some(
                p.signed_contract
                    .accepted_contract
                    .offered_contract
                    .contract_info[0]
                    .oracle_announcements[0]
                    .clone(),
            ),
            Contract::Closed(_) => None,
            Contract::Refunded(r) => Some(
                r.accepted_contract.offered_contract.contract_info[0].oracle_announcements[0]
                    .clone(),
            ),
            Contract::FailedAccept(f) => {
                Some(f.offered_contract.contract_info[0].oracle_announcements[0].clone())
            }
            Contract::FailedSign(f) => Some(
                f.accepted_contract.offered_contract.contract_info[0].oracle_announcements[0]
                    .clone(),
            ),
            Contract::Rejected(r) => Some(r.contract_info[0].oracle_announcements[0].clone()),
        }
    }

    /// Get the CET transaction id.
    pub fn get_cet_txid(&self) -> Option<Txid> {
        match self {
            Contract::Offered(_) => None,
            Contract::Accepted(_) => None,
            Contract::Signed(_) => None,
            Contract::Confirmed(_) => None,
            Contract::PreClosed(p) => Some(p.signed_cet.compute_txid()),
            Contract::Closed(c) => c.signed_cet.as_ref().map(|cet| cet.compute_txid()),
            Contract::Refunded(_) => None,
            Contract::FailedAccept(_) => None,
            Contract::FailedSign(_) => None,
            Contract::Rejected(_) => None,
        }
    }
}

/// Information about a contract that failed while verifying an accept message.
#[derive(Clone)]
pub struct FailedAcceptContract {
    /// The offered contract that was accepted.
    pub offered_contract: offered_contract::OfferedContract,
    /// The received accept message.
    pub accept_message: AcceptDlc,
    /// The error message that was generated.
    pub error_message: String,
}

/// Information about a contract that failed while verifying a sign message.
#[derive(Clone)]
pub struct FailedSignContract {
    /// The accepted contract that was signed.
    pub accepted_contract: accepted_contract::AcceptedContract,
    /// The sign message that was received.
    pub sign_message: SignDlc,
    /// The error message that was generated.
    pub error_message: String,
}

/// Information about a contract that is almost closed by a broadcasted, but not confirmed CET.
#[derive(Clone)]
pub struct PreClosedContract {
    /// The signed contract that was closed.
    pub signed_contract: SignedContract,
    /// The attestations that were used to decrypt the broadcast CET.
    pub attestations: Option<Vec<OracleAttestation>>,
    /// The signed version of the CET that was broadcast.
    pub signed_cet: Transaction,
}

/// Information about a contract that was closed by a CET that was confirmed on the blockchain.
#[derive(Clone)]
pub struct ClosedContract {
    /// The attestations that were used to decrypt the broadcast CET.
    pub attestations: Option<Vec<OracleAttestation>>,
    /// The signed version of the CET that was broadcast.
    pub signed_cet: Option<Transaction>,
    /// The id of the contract
    pub contract_id: ContractId,
    /// The temporary id of the contract.
    pub temporary_contract_id: ContractId,
    /// The public key of the counter-party's node.
    pub counter_party_id: PublicKey,
    /// The funding txid of the contract.
    pub funding_txid: Txid,
    /// The profit and loss for the given contract
    pub pnl: SignedAmount,
}

/// Information about the adaptor signatures and the CET for which they are
/// valid.
#[derive(Clone)]
pub enum AdaptorInfo {
    /// For enumeration outcome DLC, no special information needs to be kept.
    Enum,
    /// For numerical outcome DLC, a trie is used to store the information.
    Numerical(MultiOracleTrie),
    /// For numerical outcome DLC where oracles are allowed to diverge to some
    /// extent in the outcome value, a trie of trie is used to store the information.
    NumericalWithDifference(MultiOracleTrieWithDiff),
}

/// The descriptor of a contract.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "use-serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "camelCase")
)]
pub enum ContractDescriptor {
    /// Case for enumeration outcome DLC.
    Enum(enum_descriptor::EnumDescriptor),
    /// Case for numerical outcome DLC.
    Numerical(numerical_descriptor::NumericalDescriptor),
}

impl ContractDescriptor {
    /// Get the parameters on allowed divergence between oracle if any.
    pub fn get_oracle_params(&self) -> Option<numerical_descriptor::DifferenceParams> {
        match self {
            ContractDescriptor::Enum(_) => None,
            ContractDescriptor::Numerical(n) => n.difference_params.clone(),
        }
    }

    /// Validate that all possible outcomes that can be attested by the oracle(s)
    /// have a single associated payout.
    pub fn validate(
        &self,
        announcements: &Vec<OracleAnnouncement>,
    ) -> Result<(), crate::error::Error> {
        let first = announcements
            .first()
            .expect("to have at least one element.");
        match &first.oracle_event.event_descriptor {
            EventDescriptor::EnumEvent(ee) => {
                for announcement in announcements {
                    match &announcement.oracle_event.event_descriptor {
                        EventDescriptor::EnumEvent(enum_desc) => {
                            if !unordered_equal(&ee.outcomes, &enum_desc.outcomes) {
                                return Err(Error::InvalidParameters(
                                    "Oracles don't have same enum outcomes.".to_string(),
                                ));
                            }
                        }
                        _ => {
                            return Err(Error::InvalidParameters(
                                "Expected enum event descriptor.".to_string(),
                            ))
                        }
                    }
                }
                match self {
                    ContractDescriptor::Enum(ed) => ed.validate(ee),
                    _ => Err(Error::InvalidParameters(
                        "Event descriptor from contract and oracle differ.".to_string(),
                    )),
                }
            }
            EventDescriptor::DigitDecompositionEvent(_) => match self {
                ContractDescriptor::Numerical(n) => {
                    let min_nb_digits = n.oracle_numeric_infos.get_min_nb_digits();
                    let max_value = n
                        .oracle_numeric_infos
                        .base
                        .checked_pow(min_nb_digits as u32)
                        .ok_or_else(|| {
                            Error::InvalidParameters("Could not compute max value".to_string())
                        })?;
                    n.validate((max_value - 1) as u64)
                }
                _ => Err(Error::InvalidParameters(
                    "Event descriptor from contract and oracle differ.".to_string(),
                )),
            },
        }
    }
}
