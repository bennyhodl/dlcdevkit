//! Per-outcome payout scripts carried on a DLC offer as a TLV record.
//!
//! A CET pays the offerer's script and the accepter's script and nothing else. A
//! lending product settled by a DLC needs an outcome that pays a third party: the
//! position was liquidated, and the liquidator who paid for it on the EVM side is owed
//! the Bitcoin. The offer message has no field for that script, and both parties must
//! build the same CET bytes or the adaptor signatures do not verify.
//!
//! [`PayoutScriptOverrides`] names the enum outcomes whose CET pays somewhere other
//! than the accepter. For each named outcome, the output that would have paid the
//! accepter's `payout_spk` pays `script_pubkey` instead. Amounts are untouched, so an
//! outcome that should pay the whole collateral to the third party is written with
//! `offer = 0, accept = total` in the contract descriptor, as it would be for the
//! accepter. In a loan the accepter is the lender, and a liquidator is paid in the
//! lender's place, so the lender's own liquidation outcome needs no override.
//!
//! ## Type range
//!
//! [`PAYOUT_SCRIPT_OVERRIDES_TYPE`] is in the odd, application-assigned range. The DLC
//! specification assigns nothing there, so a record at this type cannot collide with one
//! a future specification version defines, and a peer that does not know it carries it
//! through untouched rather than rejecting the offer. A peer that does not apply it
//! builds different CETs and fails signature verification, which is the safe failure.

use bitcoin::ScriptBuf;

/// The TLV record type of [`PayoutScriptOverrides`], in the odd application range.
pub const PAYOUT_SCRIPT_OVERRIDES_TYPE: u16 = 65005;

/// One enum outcome whose CET pays `script_pubkey` in place of the offerer's script.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct PayoutScriptOverride {
    /// The outcome string exactly as it appears in the enum descriptor and the oracle
    /// announcement, for example `liquidated-by-0x…`.
    pub outcome: String,
    /// The script the accepter's output pays for this outcome.
    pub script_pubkey: ScriptBuf,
}

impl_dlc_writeable!(PayoutScriptOverride, {
    (outcome, string),
    (script_pubkey, writeable)
});

/// The outcomes on an offer whose CETs pay a script other than the offerer's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct PayoutScriptOverrides {
    /// The overrides, one per outcome.
    pub overrides: Vec<PayoutScriptOverride>,
}

impl_dlc_writeable!(PayoutScriptOverrides, { (overrides, vec) });
impl_dlc_tlv_record!(PayoutScriptOverrides, PAYOUT_SCRIPT_OVERRIDES_TYPE);

impl PayoutScriptOverrides {
    /// Returns the script for `outcome`, if the record names it.
    pub fn script_for(&self, outcome: &str) -> Option<&ScriptBuf> {
        self.overrides
            .iter()
            .find(|o| o.outcome == outcome)
            .map(|o| &o.script_pubkey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ser_impls::TlvRecord;
    use crate::tlv_stream::TlvStream;
    use lightning::io::Cursor;
    use lightning::util::ser::Writeable;

    fn record() -> PayoutScriptOverrides {
        PayoutScriptOverrides {
            overrides: vec![PayoutScriptOverride {
                outcome: "liquidated-by-0x1111111111111111111111111111111111111111".to_string(),
                script_pubkey: ScriptBuf::from_bytes(vec![0x00, 0x14, 0xaa, 0xbb]),
            }],
        }
    }

    /// The bytes node-dlc writes for `record()`. Pinned on both sides so a
    /// change to either encoder fails here rather than at signature
    /// verification.
    #[test]
    fn encodes_to_the_same_bytes_as_node_dlc() {
        let outcome = "liquidated-by-0x1111111111111111111111111111111111111111";
        let mut wire = vec![0xfd, 0xfd, 0xed, 0x40, 0x01, 0x38];
        wire.extend_from_slice(outcome.as_bytes());
        wire.extend_from_slice(&[0x00, 0x04, 0x00, 0x14, 0xaa, 0xbb]);

        assert_eq!(record().to_tlv_bytes(), wire);
    }

    #[test]
    fn round_trips_through_a_stream() {
        let mut stream = TlvStream::default();
        stream.set(&record());
        let decoded = TlvStream::read_to_end(&mut Cursor::new(stream.encode())).unwrap();
        assert_eq!(
            decoded.get::<PayoutScriptOverrides>().unwrap(),
            Some(record())
        );
    }

    #[test]
    fn script_for_finds_only_named_outcomes() {
        let record = record();
        assert!(record
            .script_for("liquidated-by-0x1111111111111111111111111111111111111111")
            .is_some());
        assert!(record.script_for("released").is_none());
    }
}
