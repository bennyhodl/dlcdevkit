---
title: "Custom TLV Streams in DLC Messages"
type: plan
updated: "2026-08-13"
tags: [dlcdevkit, ddk-messages, tlv, serialization, interop, node-dlc, bal, stateless-contracts]
status: partially implemented
related: [[docs/oracle-message-serialization.md]]
---

# Custom TLV Streams in DLC Messages

> **Goal:** Let an application attach its own TLV records to a DLC message, read them
> back off a message it receives, and have records it does not understand survive
> a round trip instead of being silently discarded.
>
> **Scope:** `ddk-messages` and the stateless `ddk::contract` module. The manager
> path benefits without being touched.
>
> **Prerequisite:** landed. The TLV primitives this plan builds on
> (`TlvType`, `TlvRecord`, `read_tlv_body`) shipped with the oracle message
> serialization fix. See [[docs/oracle-message-serialization.md]].

---

## 1. Why We're Doing This

### 1.1 Today's pain

DDK silently drops application TLV records. Verified by appending one record
(type 64999, 3-byte value, 7 bytes total) to a real `SignDlc` and reading it back:

```
read ok        = true
consumed       19651 of 19658
re-serialized  19651   ← the 7 bytes are gone
```

The message parses without complaint. `impl_dlc_writeable!` reads the declared
fields, stops, and leaves the trailing bytes unread; writing produces the fields
again and nothing else. There is no error and no warning at any layer. Nothing
downstream can tell that data was lost.

This matters concretely because our own counterparties send such records.
node-dlc appends a TLV stream to `DlcSign`, `DlcOffer` and `DlcAccept` per
dlcspecs PR #163, parsing what it knows (`BatchFundingGroup`) and retaining
everything else:

```ts
// packages/messaging/lib/messages/DlcSign.ts — deserialize
while (!reader.eof) {
  const buf = getTlv(reader);
  const { type } = deserializeTlv(new BufferReader(buf));
  switch (Number(type)) {
    case MessageType.BatchFundingGroup: /* parse */ break;
    default:
      instance.unknownTlvs.push({ type: Number(type), data: buf });
  }
}

// serialize
if (this.unknownTlvs) {
  this.unknownTlvs.forEach((tlv) => writer.writeBytes(tlv.data));
}
```

So a message that passes through DDK loses records a BAL peer expects to get
back, and DDK applications have no way to carry their own context (a loan id, a
lender reference) alongside a contract.

### 1.2 The target state

An application defines a record type in its own crate and uses it directly on
the message structs the stateless API already hands it:

```rust
pub const LOAN_REF_TYPE: u16 = 65003;   // odd, application range

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoanRef {
    pub loan_id: [u8; 16],
    pub lender: String,
}

impl_dlc_writeable!(LoanRef, { (loan_id, writeable), (lender, string) });
impl_dlc_tlv_record!(LoanRef, LOAN_REF_TYPE);
```

```rust
// offering side
let mut offer = ddk::contract::create_offer(params)?;
offer.tlvs.set(&LoanRef { loan_id, lender });
// the application already persists the message; the record rides along

// accepting side
let loan_ref: Option<LoanRef> = offer.tlvs.get()?;
```

And a record DDK does not recognise is preserved verbatim rather than dropped.

### 1.3 Non-goals

- **Interpreting unknown records.** They are held as `(type, bytes)` and written
  back unchanged. DDK does not validate them.
- **Manager or transport API.** No builder hook, no message callback, no change
  to `send_dlc_offer`. See §3 for why this is not a compromise.
- **Contract persistence.** Records do not ride `OfferedContract` →
  `AcceptedContract` → `SignedContract`. See §3.
- **Parsing node-dlc's `BatchFundingGroup`.** It becomes an unknown record that
  round-trips correctly. Giving it a typed field is separate work.

---

## 2. What Already Exists

The oracle serialization work put three of the four required pieces in place.

| Piece | Where | Status |
|---|---|---|
| Length-bounded body reads | `ser_impls::read_tlv_body` via `FixedLengthReader` | done |
| Compile-time record type for dispatch | `ser_impls::TlvType::TYPE_ID` | done |
| Standalone encode/decode per record | `ser_impls::TlvRecord`, blanket impl | done |
| Stream container + message field | — | this plan |

`read_tlv_body` is the load-bearing one. Before it, `read_as_tlv` discarded the
declared length, so there was no way to skip a record whose shape you do not
know — which is exactly what node-dlc's `getTlv(reader)` does. Everything here
depends on being able to consume an unknown record by its length alone.

The message boundary that "read until EOF" needs already exists too:
`message_handler.rs:182` wraps the body in `FixedLengthReader::new(&mut buf, remaining)`
before dispatching, and the nostr path reads from a cursor over exactly the
message bytes.

---

## 3. Why the Stateless Module Is the Right Home

This was the decision that made the work small.

Through the **manager**, an application never touches a raw message.
`send_dlc_offer` builds the offer inside the manager actor and the transport
sends it — the `OfferDlc` comes back after it has already gone out, so there is
nowhere to attach anything. `Transport::start` hands an incoming message straight
to `manager.on_dlc_message`, so nothing sees it on the way in. What the
application eventually reads is an `OfferedContract` out of storage. Supporting
custom records there would mean a builder hook, a receive callback, and threading
the records through every contract struct so they survive to storage — which
changes `contract_data` and needs a migration.

Through the **stateless `ddk::contract` module**, none of that applies. Its
premise is already that "the three wire messages are the authoritative state":
`create_offer` returns an owned `OfferDlc`, `accept_offer` takes `&OfferDlc`, and
the application persists the messages itself. Put a field on the message and the
application can use it — no new API of any kind.

The manager path still gains relay fidelity for free: it stops discarding records
it does not understand. It just cannot surface them to an application, which is
acceptable because it is not the module an application would use for this.

**`OfferedContract` does not embed `OfferDlc`** (`ddk-manager/src/contract/offered_contract.rs:26-55`);
it decomposes the offer into its own fields. So a field on the message never
reaches `contract_data`, and `stored_contracts_round_trip_byte_for_byte` keeps
passing. No migration.

---

## 4. Design

### 4.1 `TlvStream`

Landed as `dlc-messages/src/tlv_stream.rs`, with `TlvStreamRecord { tlv_type: u64,
body: Vec<u8> }` holding the body alone; the header is rewritten from the type and
`body.len()`. `is_empty`, `read_to_end`, `get`, `set`, `remove` and `raw` are as
sketched here.

`get`/`set` key off `TlvRecord::TYPE_ID`, so there is no runtime registry to
populate and no way to read a record at the wrong type.

**Records are held in wire order, not sorted, and duplicates are kept.** Duplicates
are forced by the peer: node-dlc appends one `BatchFundingGroup` record per group, so
rejecting duplicates would reject messages that are valid today. Wire order is our own
choice — a message we read and wrote back should be the message we received, and that
is also what makes `write` deterministic for the byte-equality comparisons.

Wire order is **not** a parity claim. node-dlc's `DlcOffer` decodes the types it knows
into named fields and re-emits the stream in a fixed order (metadata, IRC info,
position info, batch funding groups, then unknown records) rather than the order it
read, and keeps only the last of a repeated singleton type where `get` here returns the
first. Neither difference loses a record, and nothing on either side derives an
identifier or a signature from the encoded offer — `temporaryContractId` is a wire
field on both stacks, not a hash of the message — so the divergence is confined to byte
order. Anything that later hashes these bytes has to revisit it. Separately, node-dlc's
`DlcOffer.toJSON()` builds a `tlvs` array and then omits it from the returned object,
so offers must cross the boundary as serialized bytes, not as JSON.

### 4.2 Message field

A new field kind in `impl_dlc_writeable!`, valid only as the final field:

```rust
impl_dlc_writeable!(SignDlc, SIGN_TYPE, {
    (protocol_version, writeable),
    (contract_id, writeable),
    (cet_adaptor_signatures, writeable),
    (refund_signature, writeable),
    (funding_signatures, writeable),
    (tlvs, tlv_stream)              // must be last
});
```

`field_write!` writes every record; `field_read!` calls `TlvStream::read_to_end`.

Applied to `OfferDlc`, `AcceptDlc` and `SignDlc` to start — the three messages
node-dlc extends and the three the stateless module exchanges. The channel
messages can follow if a need appears.

### 4.3 Compatibility

**Wire.** An empty stream writes zero bytes, so a message from a peer that uses
no records is byte-identical to today, in both directions.

**JSON.** The stateless module is what the FFI and mobile bindings use, and
`ddk-node` returns messages as JSON, so the field needs
`#[cfg_attr(feature = "use-serde", serde(default, skip_serializing_if = "TlvStream::is_empty"))]`.
Old payloads without the field parse, and messages with no records serialize
unchanged. `contract_flags` already uses this pattern at
`ddk-manager/src/contract/offered_contract.rs:51`.

Record bytes serialize as hex, matching the convention `serde_utils` already
uses for byte fields.

### 4.4 Macro hygiene

`impl_dlc_writeable!` is `#[macro_export]`ed but is not usable from another crate
as written. Its body references `Writeable`, `Writer`, `Readable`, `DecodeError`
and `lightning::io::Read` unqualified (`dlc-messages/src/ser_macros.rs:82-102`),
so a consumer needs all four in scope *and* `lightning` as a direct dependency at
exactly the version `ddk-messages` resolves. A mismatch produces a confusing
trait error far from its cause.

Since defining a record type is the first thing an application does, this has to
be fixed for the feature to be usable at all:

- Fully qualify the macro internals (`::lightning::util::ser::Writeable`, etc.).
- Re-export `lightning` and `ddk_messages` from `ddk` — today `ddk/src/lib.rs:47`
  re-exports only `ddk_manager`, so a consumer must add `ddk-messages` to their
  own manifest and keep its version in step by hand.

---

## 5. Open Decision

**Unknown even types: reject or keep?**

The Lightning convention is that an unknown even type must be rejected and an odd
one may be ignored. node-dlc keeps everything regardless of parity.

**Decided: keep everything, regardless of parity.** Interop with BAL is the point of
the feature, DDK does not validate these records, and rejecting a message mid-protocol
over a record we were never going to read is worse than carrying it. The warning on an
unknown even type was dropped with it — `ddk-messages` has no logger, and adding one to
this crate for a line nobody reads is not worth the dependency.

---

## 6. Tasks

Items 2 and 4 landed for `OfferDlc` only, along with the compatibility tests in item 5.
`OfferDlc` has hand-written `Writeable`/`Readable` impls — it is the one message the
`impl_dlc_writeable!` macro cannot express, because of the peek-based old-format branch
— so it was wired up directly and item 3 was not needed yet. `AcceptDlc` and `SignDlc`
still drop records; they go through the macro, so they need item 3 first.


1. **Macro hygiene.** Qualify paths in `impl_dlc_writeable!`, `field_write!`,
   `field_read!`. Re-export `lightning` and `ddk_messages` from `ddk`. Add a test
   crate, or a doc test, that defines a record type using only `ddk` imports —
   this is the regression guard for the whole consumer story.
2. **`TlvStream`.** The type, `read_to_end`, `get`/`set`/`remove`/`raw`, ordering,
   and the even/odd rule from §5. Unit tests: empty stream writes nothing;
   unknown records round-trip verbatim; `get` at the wrong type returns `None`;
   duplicate types kept in wire order; truncated record rejected; a stream past
   the size cap rejected rather than truncated.
3. **`tlv_stream` field kind.** Add to the field macros, rejecting use anywhere
   but last.
4. **Wire up the three messages.** `OfferDlc`, `AcceptDlc`, `SignDlc`, with the
   serde attributes from §4.3.
5. **Compatibility tests.** A message with an empty stream is byte-identical to
   the pre-change encoding; existing `test_inputs/*.json` parse unchanged;
   `stored_contracts_round_trip_byte_for_byte` still passes.
6. **node-dlc interop.** Round-trip a real `DlcSign` carrying a
   `BatchFundingGroup` record produced by node-dlc, asserting the bytes come back
   identical. This is the test that would have caught the silent drop.
7. **Stateless example.** Extend `ddk/src/contract/tests.rs` with a lifecycle that
   attaches a record on the offer and reads it on the accept side, so the
   documented consumer story is executed.
8. **Docs.** A section in [[docs/oracle-message-serialization.md]], or a sibling
   page, covering how to define a record type and the application type range.

---

## 7. Notes

- Type range: applications should use odd types in the custom range to avoid
  colliding with anything the specification assigns. The example uses 65003.
- This does not make DDK a validating relay. A record it carries has been checked
  for framing only — that its length fits inside the message — never for meaning,
  and never for uniqueness: duplicates are kept, so a caller reading a record it
  understands is reading the first of however many arrived.
- If an application later needs records to survive into the manager's stored
  contracts, that is the larger piece of work described in §3, and it does need a
  `contract_data` migration. Nothing here forecloses it.
