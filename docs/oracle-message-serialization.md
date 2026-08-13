# Oracle message serialization

An oracle announcement or attestation is written in two different, both correct,
forms. Confusing the two is what led every consumer of these crates to hand-roll
the same header-stripping workaround. This page states which form goes where, and
what a consumer holding old bytes has to do.

## The two forms

**Body form.** `Writeable`/`Readable` on `OracleAnnouncement`, `OracleEvent` and
`OracleAttestation` handle the message fields alone, with no header:

```
announcement_signature (64) || oracle_public_key (32) || oracle_event (TLV)
```

**TLV form.** The same body behind a header carrying the record type and the body
length, both as `BigSize`:

```
BigSize(55332) || BigSize(body_len) || body
```

| Message | Record type |
| --- | --- |
| `OracleAnnouncement` | 55332 |
| `OracleEvent` | 55330 |
| `OracleAttestation` | 55400 |

## Which form goes where

Inside a contract, an announcement is stored in the TLV form — but the header comes
from `write_as_tlv`, which `SingleOracleInfo` and `MultiOracleInfo` wrap the field
in, not from the `Writeable` impl. So the impl must keep writing the body alone. If
it wrote a header too, every contract would carry it twice and no previously stored
contract would load. That is the trap the earlier `oracle-msg-as-tlv` attempt fell
into, and why it had to regenerate all six contract fixtures.

Everywhere the message travels on its own — an oracle's HTTP response, a nostr
event, a `BYTEA` column, a hex string in an RPC — the TLV form is correct. It is
what other DLC implementations emit, and it is the form the announcement
signature commits to.

## Reading and writing the standalone form

Use `TlvRecord`. It exists so that nobody has to know the above:

```rust
use ddk_messages::oracle_msgs::OracleAnnouncement;
use ddk_messages::TlvRecord;

let announcement = OracleAnnouncement::from_tlv_hex(&response.hex)?;
let bytes = announcement.to_tlv_bytes();
```

Do not reach for `OracleAnnouncement::read` on bytes that arrived on their own.
It reads the body form, so it will fail on a TLV record, and stripping the two
`BigSize` fields by hand to make it work is the workaround this trait replaces.

`read_as_tlv` now checks both halves of the header: the record type must match the
type being read, and the body must consume exactly the declared number of bytes.
A truncated or mistyped record fails outright instead of decoding into a plausible
value and leaving the reader mid-record.

## Migrating stored bytes

**Contracts need no migration.** A contract's announcements were always written in
the embedded body form and read back the same way. `stored_contracts_round_trip_byte_for_byte`
in `ddk/src/util/ser.rs` reads each contract state from a fixture serialized by an
earlier release and writes it back, asserting every byte matches; a production
announcement likewise round-trips to the exact hex its oracle signed. While those
pass, a `contract_data` column holds what this code expects.

(The fixtures under `testconfig/contract_binaries/old/` are a separate matter. All
but `Offered` stopped deserializing some time ago because of unrelated changes to
the contract structs, and that was already true before any of this.)

**Standalone oracle bytes may need one.** A store that persisted announcements or
attestations on their own holds the body form if it was written through
`Writeable::encode`, the TLV form if it was written from an oracle's own bytes, and
a mix if it has seen both. `TlvRecord::from_tlv_bytes_or_legacy` reads either:

```rust
let announcement = OracleAnnouncement::from_tlv_bytes_or_legacy(&row.bytes)?;
```

The two are told apart by the header, which a body cannot imitate: a body opens
with a Schnorr signature or a string length, neither of which encodes the record
type as a `BigSize` and then accounts for the buffer exactly.

To migrate such a column, read every row with `from_tlv_bytes_or_legacy` and write
it back with `to_tlv_bytes`. Once no legacy rows remain, switch the read path to
`from_tlv_bytes` so a malformed row is reported rather than guessed at.

## Nostr events

Kormir publishes announcements (kind 88) and attestations (kind 89) as base64 of
the TLV form, so other DLC clients on the relay can read them. Events published
before this carry the body form, and a relay's history cannot be rewritten, so
readers use `from_tlv_bytes_or_legacy`.

## Adding a new type in this position

`oracle_announcement` is the only type in the specification today that is all three of
"has its own record type", "nested inside another message" and "travels standalone".
A v2 message could well be the second, so the handling is structural rather than
special-cased.

Declare the record type once:

```rust
impl_dlc_tlv_record!(MyMessage, MY_MESSAGE_TYPE);
```

That implements `TlvType` with the constant and derives `Type` from it, so the two
cannot drift apart. `TlvRecord` then arrives through its blanket impl over
`TlvType + Readable + Writeable`, so the standalone form needs no further code — there
is nothing to remember and nothing to forget. Keep the `Writeable` impl body-only, and
nest the field with `write_as_tlv`/`read_as_tlv` as usual.

Two things this macro is not for:

- **Peer-to-peer protocol messages.** `OfferDlc`, `AcceptDlc`, `SignDlc` and the channel
  messages are *wire messages*: a `u16` type and the body, with no length. That is a
  different framing, and the `Message` enum plus the wire read/write path already add
  and strip the prefix in one place — which is precisely why those types never grew this
  problem despite also being body-only. Give them a bare `Type` impl.
- **Types that never travel alone.** `EventDescriptor` is nested with a header but is
  never standalone, so nobody ever tried to read one by itself. Declaring the record
  type costs nothing, but the standalone methods will simply go unused.

Before relying on `from_tlv_bytes_or_legacy` for a new type, check its precondition:
the body must not be able to begin with the record's own type id as a `BigSize` followed
by a length that accounts for the buffer exactly. It holds for the oracle messages,
whose bodies open with a Schnorr signature or a string length.
