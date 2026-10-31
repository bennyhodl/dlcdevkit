//! BIP-329 label text for a contract's transactions.
//!
//! A wallet that labels its contracts writes the text from here, so an export
//! from one DDK wallet reads the same as an export from any other. DDK's own
//! wallet writes these labels when it syncs; a stateless wallet writes them
//! where it records a funding or a close. The labels are plain strings with
//! the bitcoin reference they belong on, so they need no label crate and work
//! without the `manager` feature: wrap them in whatever BIP-329 record type
//! the wallet stores.
//!
//! A contract id is written as the lowercase hex of its 32 bytes, in the
//! order they appear on the wire.

use bitcoin::{OutPoint, Txid};
use ddk_manager::ContractId;

/// The two labels a contract's funding writes: the funding transaction gets
/// one naming the contract, and the funding output gets the contract id alone,
/// so the coin can be traced back to its contract by id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FundingLabels {
    /// The funding transaction, which `transaction_label` belongs on.
    pub txid: Txid,
    /// `DLC funding <contract id>`.
    pub transaction_label: String,
    /// The 2-of-2 funding output, which `output_label` belongs on.
    pub outpoint: OutPoint,
    /// `<contract id>`.
    pub output_label: String,
}

/// The labels for the funding of `contract_id`, whose 2-of-2 output is
/// `outpoint`.
pub fn funding_labels(contract_id: &ContractId, outpoint: OutPoint) -> FundingLabels {
    let id = hex::encode(contract_id);
    FundingLabels {
        txid: outpoint.txid,
        transaction_label: format!("DLC funding {id}"),
        outpoint,
        output_label: id,
    }
}

/// The label for the transaction that closed `contract_id`:
/// `DLC close <contract id>: <outcomes>`, with the attested outcomes joined
/// by commas, or `DLC close <contract id>` when they join to nothing (no
/// attestations, or a close whose attestations the wallet does not have).
pub fn close_label(contract_id: &ContractId, outcomes: &[String]) -> String {
    let id = hex::encode(contract_id);
    let outcomes = outcomes.join(",");
    if outcomes.is_empty() {
        format!("DLC close {id}")
    } else {
        format!("DLC close {id}: {outcomes}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;

    /// The label text is what other wallets read in an export, so it is
    /// pinned here: changing it must be a deliberate decision.
    #[test]
    fn label_text_is_stable() {
        // Distinct bytes, so a reversed or truncated id would show.
        let contract_id: ContractId = std::array::from_fn(|i| i as u8);
        let id = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
        let outpoint = OutPoint {
            txid: Txid::from_byte_array([7; 32]),
            vout: 1,
        };

        assert_eq!(
            funding_labels(&contract_id, outpoint),
            FundingLabels {
                txid: outpoint.txid,
                transaction_label: format!("DLC funding {id}"),
                outpoint,
                output_label: id.to_string(),
            }
        );
        assert_eq!(
            close_label(&contract_id, &["BTC".to_string(), "up".to_string()]),
            format!("DLC close {id}: BTC,up")
        );
        assert_eq!(close_label(&contract_id, &[]), format!("DLC close {id}"));
        // Outcomes that join to nothing write no outcome text at all.
        assert_eq!(
            close_label(&contract_id, &[String::new()]),
            format!("DLC close {id}")
        );
        assert_eq!(
            close_label(&contract_id, &[String::new(), String::new()]),
            format!("DLC close {id}: ,")
        );
    }
}
