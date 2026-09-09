//! The TLV stream a DLC message may carry after its fixed fields, per
//! [dlcspecs PR #163]. Records keep the order they were read in, including
//! duplicates and unknown types, and are written back byte for byte, so a
//! record a peer appended survives a round trip. An empty stream writes no
//! bytes, so a message without records encodes the same as it did before this
//! type existed. node-dlc keeps records too but re-emits them in its own fixed
//! order, so do not hash these bytes.
//!
//! [dlcspecs PR #163]: https://github.com/discreetlogcontracts/dlcspecs/pull/163

use crate::ser_impls::{read_tlv_body, BigSize, TlvRecord};
use lightning::io::{Cursor, Read};
use lightning::ln::msgs::DecodeError;
use lightning::util::ser::{Readable, Writeable, Writer};

/// Upper bound on the bytes a TLV stream may occupy. The rest of the crate caps
/// variable-length reads the same way so a peer cannot make us allocate freely.
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
    /// The record type, as read from the wire. It is wider than the `u16` a
    /// [`TlvType`](crate::TlvType) declares because a record we do not
    /// recognise may use a type this build has no constant for.
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
    /// Only valid at the end of a message, where the remaining bytes are the
    /// stream and nothing else. Every caller in this crate reads a message from
    /// a reader bounded to that message's bytes, so the read stops at the end
    /// of the message. An empty remainder is an empty stream, not an error, so
    /// messages from peers that append nothing still read.
    pub fn read_to_end<R: Read>(reader: &mut R) -> Result<Self, DecodeError> {
        let mut bytes = Vec::new();
        // Read one byte past the cap. `read_to_limit` stops at its limit and
        // returns Ok, so reading exactly the cap could not tell a full stream
        // from a truncated one.
        reader
            .read_to_limit(&mut bytes, MAX_TLV_STREAM_SIZE + 1)
            .map_err(|_| DecodeError::ShortRead)?;
        if bytes.len() as u64 > MAX_TLV_STREAM_SIZE {
            return Err(DecodeError::InvalidValue);
        }

        let len = bytes.len() as u64;
        let mut cursor = Cursor::new(bytes);
        let mut records = Vec::new();

        while cursor.position() < len {
            let tlv_type: BigSize = Readable::read(&mut cursor)?;
            let body_len: BigSize = Readable::read(&mut cursor)?;
            // Check before allocating. A record may declare any length,
            // including one far past the end of the message.
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

    /// Writes `value`, replacing every record already held at `T::TYPE_ID`.
    ///
    /// The replacement keeps the position of the first one, so setting a record that came
    /// off the wire does not move it relative to the records around it.
    pub fn set<T: TlvRecord>(&mut self, value: &T) {
        let record = TlvStreamRecord {
            tlv_type: T::TYPE_ID as u64,
            body: value.encode(),
        };
        match self.position(T::TYPE_ID) {
            // Remove every duplicate, not just the first. A read keeps
            // duplicates, so a stale copy would stay on the wire and the peer
            // could read it instead of the new record.
            Some(index) => {
                self.records.retain(|r| r.tlv_type != T::TYPE_ID as u64);
                self.records.insert(index, record);
            }
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
        // Sorting on write would mean a message we read and wrote back was not the
        // message we received, which is the property the rest of this module exists to
        // hold. It is not a parity claim: node-dlc re-emits in its own fixed order (see
        // the module docs), so bytes are only stable across a DDK-to-DDK hop.
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
    fn stream_past_the_size_cap_is_rejected_not_truncated() {
        // `read_to_limit` stops at its limit and returns Ok, so reading exactly
        // the cap would decode the first megabyte and drop the rest with no
        // error. That is the same silent loss this module exists to stop.
        let record: &[u8] = &[0x01, 0x63, 0x00];
        let mut bytes = Vec::new();
        while (bytes.len() as u64) <= MAX_TLV_STREAM_SIZE {
            bytes.extend_from_slice(record);
            bytes.resize(bytes.len() + 99, 0);
        }
        assert!(bytes.len() as u64 > MAX_TLV_STREAM_SIZE);
        assert!(read(&bytes).is_err());
    }

    #[test]
    fn set_replaces_every_duplicate_and_keeps_the_first_position() {
        // A stream off the wire may hold duplicates, so replacing only the first would
        // leave a stale record behind for the peer to read instead.
        let mut bytes = vec![0x01, 0x01, 0xaa];
        bytes.extend_from_slice(UNKNOWN_RECORD);
        bytes.extend_from_slice(&[0x01, 0x01, 0xbb]);
        let mut stream = read(&bytes).unwrap();
        assert_eq!(stream.raw().count(), 3);

        stream.set(&OneByte(0xcc));

        assert_eq!(stream.raw().count(), 2);
        assert_eq!(stream.encode(), {
            let mut expected = vec![0x01, 0x01, 0xcc];
            expected.extend_from_slice(UNKNOWN_RECORD);
            expected
        });
    }

    /// A one-byte record at type 1, so `set` has a `TlvRecord` to write.
    struct OneByte(u8);

    impl Writeable for OneByte {
        fn write<W: Writer>(&self, w: &mut W) -> Result<(), lightning::io::Error> {
            self.0.write(w)
        }
    }

    impl Readable for OneByte {
        fn read<R: Read>(r: &mut R) -> Result<Self, DecodeError> {
            Ok(OneByte(Readable::read(r)?))
        }
    }

    impl crate::ser_impls::TlvType for OneByte {
        const TYPE_ID: u16 = 1;
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
