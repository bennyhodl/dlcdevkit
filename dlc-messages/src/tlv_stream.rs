//! The TLV stream that a DLC message may carry after its fixed fields.
//!
//! A DLC message is a fixed sequence of positionally encoded fields. Anything an
//! application wants to attach to one — a loan reference, liquidation terms, a batch
//! funding group — has nowhere to live in that layout, so [dlcspecs PR #163] appends a
//! stream of TLV records to the end of the message instead.
//!
//! ## Why records are kept verbatim
//!
//! A reader that stops after the last fixed field leaves the trailing records unread and
//! writes the message back out without them. Nothing errors, so nothing downstream can
//! tell the data is gone. Our own counterparties append these records: node-dlc keeps
//! what it does not recognise in `unknownTlvs` and re-emits it, so a message that passes
//! through DDK today loses records the peer expects to get back.
//!
//! [`TlvStream`] therefore holds every record it reads, in the order it read them, and
//! writes them back byte for byte. Records this build has a type for are still readable
//! as that type through [`TlvStream::get`]; the rest simply survive.
//!
//! ## Compatibility
//!
//! An empty stream writes zero bytes, so a message from a peer that uses no records is
//! byte-identical to one encoded before this type existed, in both directions. That is
//! what makes the field safe to add to an existing message without a version gate.
//!
//! [dlcspecs PR #163]: https://github.com/discreetlogcontracts/dlcspecs/pull/163

use crate::ser_impls::{read_tlv_body, BigSize, TlvRecord};
use lightning::io::{Cursor, Read};
use lightning::ln::msgs::DecodeError;
use lightning::util::ser::{Readable, Writeable, Writer};

/// Upper bound on the bytes a TLV stream may occupy, mirroring the cap the rest of this
/// crate puts on variable-length reads so a hostile peer cannot make us allocate freely.
const MAX_TLV_STREAM_SIZE: u64 = 1_000_000;

/// A single TLV record, with its body held as raw bytes.
///
/// The body excludes the record's own type and length header; [`TlvStream`] writes those
/// back from `tlv_type` and `body.len()`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct TlvStreamRecord {
    /// The record type, as read from the wire. Wider than the `u16` a
    /// [`TlvType`](crate::TlvType) declares, because a record we do not recognise may
    /// legitimately use a type this build has no constant for.
    pub tlv_type: u64,
    #[cfg_attr(
        feature = "use-serde",
        serde(
            serialize_with = "crate::serde_utils::serialize_hex",
            deserialize_with = "crate::serde_utils::deserialize_hex_string"
        )
    )]
    /// The record body, exactly as it appeared on the wire.
    pub body: Vec<u8>,
}

/// The TLV records at the end of a message, in wire order.
///
/// See the [module documentation](self) for why order is preserved and what an empty
/// stream encodes to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(transparent)
)]
pub struct TlvStream {
    records: Vec<TlvStreamRecord>,
}

impl TlvStream {
    /// Returns whether the stream holds no records, in which case it encodes to no bytes.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Reads records until the reader is exhausted.
    ///
    /// Only valid at the end of a message, where the remaining bytes are the stream and
    /// nothing else. Every caller in this crate reads a message from a reader bounded to
    /// that message's bytes — a [`FixedLengthReader`](lightning::util::ser::FixedLengthReader)
    /// in the peer-message path, a cursor over the exact payload elsewhere — so "until
    /// exhausted" means "to the end of this message", not "to the end of the connection".
    ///
    /// An empty remainder yields an empty stream rather than an error, which is what keeps
    /// messages from peers that append nothing readable.
    pub fn read_to_end<R: Read>(reader: &mut R) -> Result<Self, DecodeError> {
        let mut bytes = Vec::new();
        reader
            .read_to_limit(&mut bytes, MAX_TLV_STREAM_SIZE)
            .map_err(|_| DecodeError::ShortRead)?;

        let len = bytes.len() as u64;
        let mut cursor = Cursor::new(bytes);
        let mut records = Vec::new();

        while cursor.position() < len {
            let tlv_type: BigSize = Readable::read(&mut cursor)?;
            let body_len: BigSize = Readable::read(&mut cursor)?;
            // Checked before allocating: a record may declare any length, including one
            // far past the end of the message.
            if body_len.0 > len - cursor.position() {
                return Err(DecodeError::ShortRead);
            }
            let mut body = vec![0u8; body_len.0 as usize];
            cursor
                .read_exact(&mut body)
                .map_err(|_| DecodeError::ShortRead)?;
            records.push(TlvStreamRecord {
                tlv_type: tlv_type.0,
                body,
            });
        }

        Ok(Self { records })
    }

    /// Reads the first record whose type is `T::TYPE_ID`, if the stream holds one.
    ///
    /// Returns an error only if the record is present but its body does not decode as `T`.
    pub fn get<T: TlvRecord>(&self) -> Result<Option<T>, DecodeError> {
        let record = match self.find(T::TYPE_ID) {
            Some(record) => record,
            None => return Ok(None),
        };
        let mut cursor = Cursor::new(&record.body);
        read_tlv_body(&mut cursor, record.body.len() as u64).map(Some)
    }

    /// Writes `value`, replacing any record already held at `T::TYPE_ID`.
    pub fn set<T: TlvRecord>(&mut self, value: &T) {
        let record = TlvStreamRecord {
            tlv_type: T::TYPE_ID as u64,
            body: value.encode(),
        };
        match self.position(T::TYPE_ID) {
            Some(index) => self.records[index] = record,
            None => self.records.push(record),
        }
    }

    /// Removes every record held at `tlv_type`, returning whether any were held.
    pub fn remove(&mut self, tlv_type: u16) -> bool {
        let before = self.records.len();
        self.records.retain(|r| r.tlv_type != tlv_type as u64);
        self.records.len() != before
    }

    /// Iterates every record, including those this build has no type for.
    pub fn raw(&self) -> impl Iterator<Item = &TlvStreamRecord> {
        self.records.iter()
    }

    fn find(&self, tlv_type: u16) -> Option<&TlvStreamRecord> {
        self.records.iter().find(|r| r.tlv_type == tlv_type as u64)
    }

    fn position(&self, tlv_type: u16) -> Option<usize> {
        self.records
            .iter()
            .position(|r| r.tlv_type == tlv_type as u64)
    }
}

impl Writeable for TlvStream {
    fn write<W: Writer>(&self, w: &mut W) -> Result<(), lightning::io::Error> {
        for record in &self.records {
            BigSize(record.tlv_type).write(w)?;
            BigSize(record.body.len() as u64).write(w)?;
            w.write_all(&record.body)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Type 65003, 3-byte body; the shape of the record that reproduced the silent drop.
    const UNKNOWN_RECORD: &[u8] = &[0xfd, 0xfd, 0xeb, 0x03, 0x01, 0x02, 0x03];

    fn read(bytes: &[u8]) -> Result<TlvStream, DecodeError> {
        TlvStream::read_to_end(&mut Cursor::new(bytes.to_vec()))
    }

    #[test]
    fn empty_remainder_is_an_empty_stream_not_an_error() {
        let stream = read(&[]).unwrap();
        assert!(stream.is_empty());
        assert!(stream.encode().is_empty());
    }

    #[test]
    fn unknown_record_round_trips_verbatim() {
        let stream = read(UNKNOWN_RECORD).unwrap();
        assert_eq!(stream.raw().count(), 1);
        assert_eq!(stream.encode(), UNKNOWN_RECORD);
    }

    #[test]
    fn records_keep_wire_order_even_when_types_descend() {
        // node-dlc writes records in the order it holds them, not sorted by type, so
        // re-ordering on write would break byte equality against a real peer's message.
        let mut bytes = vec![0xfd, 0xfd, 0xec, 0x01, 0xaa];
        bytes.extend_from_slice(&[0x01, 0x01, 0xbb]);
        assert_eq!(read(&bytes).unwrap().encode(), bytes);
    }

    #[test]
    fn duplicate_types_are_kept() {
        // node-dlc appends one BatchFundingGroup record per group, so rejecting
        // duplicates would reject messages from peers that are valid today.
        let mut bytes = UNKNOWN_RECORD.to_vec();
        bytes.extend_from_slice(UNKNOWN_RECORD);
        let stream = read(&bytes).unwrap();
        assert_eq!(stream.raw().count(), 2);
        assert_eq!(stream.encode(), bytes);
    }

    #[test]
    fn record_running_past_the_end_is_rejected() {
        // Declared length 200, body 3 bytes: without the bounds check this allocates and
        // then reads short, or steals bytes that are not there.
        assert!(read(&[0xfd, 0xfd, 0xeb, 0xc8, 0x01, 0x02, 0x03]).is_err());
    }

    #[test]
    fn truncated_header_is_rejected() {
        assert!(read(&[0xfd, 0xfd]).is_err());
    }

    #[test]
    fn remove_drops_every_record_at_the_type() {
        let mut bytes = UNKNOWN_RECORD.to_vec();
        bytes.extend_from_slice(UNKNOWN_RECORD);
        let mut stream = read(&bytes).unwrap();
        assert!(stream.remove(65003));
        assert!(stream.is_empty());
        assert!(!stream.remove(65003));
    }
}
