//! Liquidation terms carried on a DLC offer as a TLV record.
//!
//! A DLC pays out to the offerer or the accepter and to nobody else. A lending product
//! built on one needs a third outcome — the position is liquidated and a liquidator, not
//! either counterparty, is paid — and the accepting side has to be able to see the terms
//! of that outcome before it signs. The offer message has no field for them.
//!
//! [`LiquidationInfo`] is those terms as a [`TlvStream`](crate::tlv_stream::TlvStream)
//! record on [`OfferDlc`](crate::OfferDlc): the announcements whose attestations decide a
//! liquidation, who gets paid when one happens, and what triggers it.
//!
//! ## Type range
//!
//! [`LIQUIDATION_INFO_TYPE`] is in the odd, application-assigned range. The DLC
//! specification assigns nothing there, so a record at this type cannot collide with one
//! a future specification version defines, and a peer that does not know it carries it
//! through untouched rather than rejecting the offer.

use crate::oracle_msgs::OracleAnnouncement;
use crate::ser_impls::{read_as_tlv, write_as_tlv};
use lightning::ln::msgs::DecodeError;
use lightning::util::ser::{Readable, Writeable, Writer};

/// The TLV record type of [`LiquidationInfo`], in the odd application range.
pub const LIQUIDATION_INFO_TYPE: u16 = 65005;

/// The length of the EVM address identifying the liquidator.
pub const EVM_ADDRESS_LEN: usize = 20;

/// What causes the contract to be liquidated.
///
/// The variant does not change how the DLC executes — every outcome is still an oracle
/// attestation over an announcement in [`LiquidationInfo::announcements`]. It tells the
/// accepting side which kind of event those announcements describe, so it can check the
/// terms it is being asked to sign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub enum LiquidationMethod {
    /// The collateral price crossing a threshold.
    Price,
    /// The oracle attesting to a named identifier, such as a missed repayment.
    Identifier,
}

impl Writeable for LiquidationMethod {
    fn write<W: Writer>(&self, w: &mut W) -> Result<(), lightning::io::Error> {
        let discriminant: u8 = match self {
            LiquidationMethod::Price => 0,
            LiquidationMethod::Identifier => 1,
        };
        discriminant.write(w)
    }
}

impl Readable for LiquidationMethod {
    fn read<R: lightning::io::Read>(r: &mut R) -> Result<Self, DecodeError> {
        // Rejected rather than defaulted: an unrecognised method means the record was
        // written by a build that liquidates on something this one cannot evaluate, and
        // treating it as `Price` would sign terms nobody agreed to.
        match u8::read(r)? {
            0 => Ok(LiquidationMethod::Price),
            1 => Ok(LiquidationMethod::Identifier),
            _ => Err(DecodeError::InvalidValue),
        }
    }
}

/// The liquidation terms attached to an offer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct LiquidationInfo {
    /// The announcements whose attestations decide a liquidation.
    ///
    /// Separate from the announcements in the contract info: those settle the contract's
    /// own payout curve, these decide whether a liquidation happened at all.
    pub announcements: Vec<OracleAnnouncement>,
    #[cfg_attr(
        feature = "use-serde",
        serde(
            serialize_with = "crate::serde_utils::serialize_hex",
            deserialize_with = "deserialize_evm_address"
        )
    )]
    /// The EVM address paid when the contract is liquidated.
    pub liquidator: [u8; EVM_ADDRESS_LEN],
    /// What causes the contract to be liquidated.
    pub method: LiquidationMethod,
}

impl_dlc_writeable!(LiquidationInfo, {
    (announcements, {vec_cb, write_as_tlv, read_as_tlv}),
    (liquidator, {cb_writeable, write_evm_address, read_evm_address}),
    (method, writeable)
});

impl_dlc_tlv_record!(LiquidationInfo, LIQUIDATION_INFO_TYPE);

/// Writes an EVM address. `lightning` has no array impl at this length.
pub fn write_evm_address<W: Writer>(
    address: &[u8; EVM_ADDRESS_LEN],
    w: &mut W,
) -> Result<(), lightning::io::Error> {
    w.write_all(address)
}

/// Reads an EVM address written by [`write_evm_address`].
pub fn read_evm_address<R: lightning::io::Read>(
    r: &mut R,
) -> Result<[u8; EVM_ADDRESS_LEN], DecodeError> {
    let mut address = [0u8; EVM_ADDRESS_LEN];
    r.read_exact(&mut address)
        .map_err(|_| DecodeError::ShortRead)?;
    Ok(address)
}

/// Deserializes an EVM address from JSON, with or without the `0x` prefix it is
/// conventionally written with.
#[cfg(feature = "use-serde")]
pub fn deserialize_evm_address<'de, D>(deserializer: D) -> Result<[u8; EVM_ADDRESS_LEN], D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    use serde::de::Error;
    use std::convert::TryInto;

    let string: String = serde::Deserialize::deserialize(deserializer)?;
    let bytes = ::bitcoin::hex::FromHex::from_hex(string.trim_start_matches("0x"))
        .map_err(D::Error::custom)
        .map(|b: Vec<u8>| b)?;
    bytes
        .try_into()
        .map_err(|_| D::Error::custom("EVM address must be 20 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tlv_stream::TlvStream;
    use crate::TlvRecord;

    fn info() -> LiquidationInfo {
        LiquidationInfo {
            announcements: Vec::new(),
            liquidator: [0xab; EVM_ADDRESS_LEN],
            method: LiquidationMethod::Identifier,
        }
    }

    #[test]
    fn record_round_trips_through_a_stream() {
        let mut stream = TlvStream::default();
        stream.set(&info());
        let bytes = stream.encode();

        let read = TlvStream::read_to_end(&mut lightning::io::Cursor::new(bytes)).unwrap();
        assert_eq!(read.get::<LiquidationInfo>().unwrap(), Some(info()));
    }

    #[test]
    fn set_replaces_rather_than_appends() {
        let mut stream = TlvStream::default();
        stream.set(&info());
        let mut updated = info();
        updated.method = LiquidationMethod::Price;
        stream.set(&updated);

        assert_eq!(stream.raw().count(), 1);
        assert_eq!(stream.get::<LiquidationInfo>().unwrap(), Some(updated));
    }

    #[test]
    fn unknown_method_is_rejected_not_defaulted() {
        let mut bytes = info().encode();
        *bytes.last_mut().unwrap() = 0xff;
        assert!(LiquidationInfo::from_tlv_bytes(&{
            let mut record = Vec::new();
            crate::ser_impls::BigSize(LIQUIDATION_INFO_TYPE as u64)
                .write(&mut record)
                .unwrap();
            crate::ser_impls::BigSize(bytes.len() as u64)
                .write(&mut record)
                .unwrap();
            record.extend_from_slice(&bytes);
            record
        })
        .is_err());
    }
}
