# ddk-messages

[![Crate](https://img.shields.io/crates/v/ddk-messages.svg?logo=rust)](https://crates.io/crates/ddk-messages)
[![Documentation](https://img.shields.io/static/v1?logo=read-the-docs&label=docs.rs&message=ddk-messages&color=informational)](https://docs.rs/ddk-messages)

Data structures and serialization for peer-to-peer communication in the Discreet Log Contract (DLC) protocol.

This crate implements the DLC specification message formats with Lightning-compatible serialization and automatic message segmentation for large messages.

## Contract Messages

| Message | Description |
|---------|-------------|
| `OfferDlc` | Initial contract offer with funding details and payouts |
| `AcceptDlc` | Contract acceptance with adaptor signatures |
| `SignDlc` | Final contract signatures |
| `CloseDlc` | Contract close message |

## Oracle Messages

| Type | Description |
|------|-------------|
| `OracleAnnouncement` | Signed announcement of an oracle event |
| `OracleAttestation` | Oracle's signed attestation to an outcome |
| `OracleEvent` | Event details (nonces, maturity, descriptor) |

## Channel Messages

| Message | Description |
|---------|-------------|
| `OfferChannel` / `AcceptChannel` / `SignChannel` | Channel setup |
| `SettleOffer` / `SettleAccept` / `SettleConfirm` | Settlement flow |
| `RenewOffer` / `RenewAccept` / `RenewConfirm` | Contract renewal |
| `CollaborativeCloseOffer` | Collaborative close |
| `Reject` | Reject a received offer |

## Message Handler

The `MessageHandler` implements LDK's `CustomMessageHandler` for integration with Lightning peer messaging:

```rust
use ddk_messages::message_handler::MessageHandler;

let handler = MessageHandler::new();

// Send a message
handler.send_message(counterparty_pubkey, Message::Offer(offer));

// Get received messages
let messages = handler.get_and_clear_received_messages();
```

## Features

| Feature | Description |
|---------|-------------|
| `std` | Standard library support (default) |
| `no-std` | No standard library support |
| `use-serde` | Serde serialization for all message types |

## License

This project is licensed under the MIT License.
