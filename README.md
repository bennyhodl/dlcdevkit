# DLC Dev Kit

A development kit to easily get started creating DLC application. Build with [`rust-dlc`](https://github.com/p2pderivatives/rust-dlc) and [`bdk`](https://github.com/bitcoindevkit/bdk).

The goal of `dlcdevkit` is to provide an easy interface to get started creating DLC applications. App developers do not have to worry
about wallets, DLC management, DLC communication, nor storage.

### crates
* `ddk` - `bdk` and `rust-dlc` library
* `bella` - a dlcdevkit example client built with `tauri`
* `payouts` - example payout curves for DLC applications

### Storage
* `sqlite`

### Transports
* `nostr`
* `lightning p2p network`
