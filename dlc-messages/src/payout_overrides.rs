//! Per-outcome payout scripts carried on a DLC offer as a TLV record.
//!
//! A CET pays the offerer's script and the accepter's script and nothing else. A
//! lending product settled by a DLC needs an outcome that pays a third party: the
//! position was liquidated, and the liquidator who paid for it on the EVM side is owed
//! the Bitcoin. The offer message has no field for that script, and both parties must
//! build the same CET bytes or the adaptor signatures do not verify.
//!
//! [`PayoutScriptOverrides`] names the outcomes whose CETs pay somewhere other than the
//! accepter: an outcome of an enum descriptor, or a range of the values of a numeric
//! one. For each CET an override covers, the output that would have paid the
//! accepter's `payout_spk` pays `script_pubkey` instead. Amounts are untouched, so an
//! outcome that should pay the whole collateral to the third party is written with
//! `offer = 0, accept = total` in the contract descriptor, as it would be for the
//! accepter. In a loan the accepter is the lender, and a liquidator is paid in the
//! lender's place, so the lender's own liquidation outcome needs no override.
//!
//! How the record changes the CETs, and what it is checked against, is the
//! `ddk-manager` CET record built on it; this module is only the wire form.
//!
//! ## Accepting an offer that carries one
//!
//! The record redirects what the accepter is paid, and it comes from the offerer. A
//! library that knows the record applies it, so an accepter must check the overrides on
//! an offer before accepting it, and decline the offer if they pay anyone it did not
//! agree to.
//!
//! ## Type number
//!
//! [`PAYOUT_SCRIPT_OVERRIDES_TYPE`] is the identifier agreed with node-dlc. It is odd,
//! so a peer that does not know the record carries it through rather than rejecting the
//! offer; that peer builds different CETs and fails signature verification, which is
//! the safe failure. It is not in a range reserved for applications: BOLT 1 starts
//! custom records at 65536, and the DLC specification could assign 65005 one day.
//! node-dlc has to use the same encoding, which the bytes pinned below fix.

use bitcoin::ScriptBuf;

/// The TLV record type of [`PayoutScriptOverrides`], as agreed with node-dlc.
pub const PAYOUT_SCRIPT_OVERRIDES_TYPE: u16 = 65005;

/// The outcomes a [`PayoutScriptOverride`] covers.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub enum OverrideOutcome {
    /// An outcome of an enum descriptor.
    Enum {
        /// The outcome string exactly as it appears in the enum descriptor and the
        /// oracle announcement, for example `liquidated-by-0x…`.
        outcome: String,
    },
    /// The values of a numeric descriptor from `start` to `end`, both included.
    Numeric {
        /// The first value covered.
        start: u64,
        /// The last value covered.
        end: u64,
    },
}

impl_dlc_writeable_enum!(OverrideOutcome,
    ;
    (0, Enum, {(outcome, string)}),
    (1, Numeric, {(start, writeable), (end, writeable)});
    ;
);

/// Outcomes whose CETs pay `script_pubkey` in place of the accepter's script.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct PayoutScriptOverride {
    /// The outcomes whose CETs the override applies to.
    pub outcome: OverrideOutcome,
    /// The script the accepter's output pays for those outcomes.
    pub script_pubkey: ScriptBuf,
}

impl_dlc_writeable!(PayoutScriptOverride, {
    (outcome, writeable),
    (script_pubkey, writeable)
});

/// The outcomes on an offer whose CETs pay a script other than the accepter's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct PayoutScriptOverrides {
    /// The overrides. No two may cover the same CET.
    pub overrides: Vec<PayoutScriptOverride>,
}

impl_dlc_writeable!(PayoutScriptOverrides, { (overrides, vec) });
impl_dlc_tlv_record!(PayoutScriptOverrides, PAYOUT_SCRIPT_OVERRIDES_TYPE);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ser_impls::TlvRecord;
    use crate::tlv_stream::TlvStream;
    use lightning::io::Cursor;
    use lightning::util::ser::Writeable;

    const OUTCOME: &str = "liquidated-by-0x1111111111111111111111111111111111111111";

    fn record() -> PayoutScriptOverrides {
        PayoutScriptOverrides {
            overrides: vec![
                PayoutScriptOverride {
                    outcome: OverrideOutcome::Enum {
                        outcome: OUTCOME.to_string(),
                    },
                    script_pubkey: ScriptBuf::from_bytes(vec![0x00, 0x14, 0xaa, 0xbb]),
                },
                PayoutScriptOverride {
                    outcome: OverrideOutcome::Numeric { start: 0, end: 499 },
                    script_pubkey: ScriptBuf::from_bytes(vec![0x00, 0x14, 0xcc]),
                },
            ],
        }
    }

    /// The bytes of `record()`, written out by hand. Another implementation
    /// has to produce exactly these, so a change to the encoder fails here
    /// rather than at signature verification.
    #[test]
    fn encodes_to_the_pinned_bytes() {
        // Type 65005 and an 87-byte body holding two overrides.
        let mut wire = vec![0xfd, 0xfd, 0xed, 0x57, 0x02];
        // An enum outcome: variant 0, the outcome string, then the script.
        wire.extend_from_slice(&[0x00, 0x38]);
        wire.extend_from_slice(OUTCOME.as_bytes());
        wire.extend_from_slice(&[0x00, 0x04, 0x00, 0x14, 0xaa, 0xbb]);
        // A numeric range: variant 1, start and end as u64, then the script.
        wire.push(0x01);
        wire.extend_from_slice(&0u64.to_be_bytes());
        wire.extend_from_slice(&499u64.to_be_bytes());
        wire.extend_from_slice(&[0x00, 0x03, 0x00, 0x14, 0xcc]);

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
}
