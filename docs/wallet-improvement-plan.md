# BDK Wallet Improvement Plan

Status of the ecosystem (2026-08): we pin `bdk_wallet 3.0.0`, `bdk_chain 0.23.3`, `bdk_esplora 0.22.2`.
The chain crates are current. `bdk_wallet 3.1.0` is out (semver-compatible).
Multi-keychain wallets and the `bdk_tx` builder target Wallet 4.0 and are not released.
The two largest wins are already inside our pinned version and unused:
**persistent outpoint locking** and **wallet events**.

---

## Phase 1 — Correctness fixes (~1 day)

These are bugs in `ddk/src/wallet/command.rs` and `ddk/src/wallet/mod.rs`.

1. **Scan both keychains on full scan.** The initial full scan only requests
   External SPKs (`command.rs:39-47`). A wallet restored from seed does not find
   its change outputs. Add `spks_for_keychain(KeychainKind::Internal, ...)`.
   Also raise `stop_gap` (10 → 50) and `parallel_requests` (1 → 5).
2. **Remove the same-height short-circuit.** `command.rs:27-29` returns early
   when the wallet tip equals the chain height. Unconfirmed transactions stay
   invisible until the next block. Always run the sync. The 60 s timer in
   `ddk.rs` already bounds the cost.
3. **Detect mempool eviction.** Seed the sync request with
   `SyncRequestBuilder::expected_spk_txids` from our unconfirmed transactions.
   BDK then stamps `evicted_at` and drops replaced/evicted transactions from the
   canonical view. Without this, a stuck balance never recovers.
4. **Delete the fabricated `last_active_indices`.** The incremental branch
   (`command.rs:68-77`) writes the current derivation indices into the update.
   A sync (not full scan) must not set last-active indices. Build the `Update`
   from the sync result alone.
5. **Bump `bdk_wallet` to 3.1.0.** No breaking changes. It fixes
   `add_foreign_utxo` non-witness validation and a panic in `Utxo::txout` for
   foreign UTXOs — both on our splice code paths.

Also in this pass: `sign_psbt_input` clones and signs the full PSBT once per
input (`mod.rs:474-503`, called in a loop by `contract_updater.rs`). Sign the
PSBT once and copy out all requested inputs, or sign in place.

## Phase 2 — UTXO reservation with BDK outpoint locking (~1 day)

The manager calls `get_utxos_for_amount(..., lock_utxos: true)` and we ignore
the flag (`mod.rs:810-815`). `unreserve_utxos` is a no-op. Two concurrent
offers can select the same coins. BDK 3.0 ships the fix:

1. In `get_utxos_for_amount`, filter `list_unspent()` through
   `Wallet::is_outpoint_locked`, run coin selection, then call
   `Wallet::lock_outpoint` on each selected outpoint when `lock_utxos` is true.
2. Implement `unreserve_utxos` with `Wallet::unlock_outpoint` (the manager
   calls it when an offer fails or a contract is rejected).
3. Locks persist through the existing `ChangeSet` (`locked_outpoints` field) —
   no storage-backend change needed; verify Postgres/Sled round-trip it.
4. Lock the funding inputs of a signed-but-unbroadcast funding transaction at
   sign time, and unlock on confirmation.
5. Split the reported balance: `spendable` = unspent minus locked;
   `reserved` = locked. Extend `crate::Balance` accordingly.

## Phase 3 — Contract UTXO tracking, separate from the wallet balance (~2-3 days)

The 2-of-2 funding outputs are invisible to BDK today. `ddk.rs::balance()`
computes the contract balance from collateral math, not from the chain. BDK
cannot show foreign outputs in `balance()`/`list_unspent()` even if inserted,
so we track them beside the wallet with the `bdk_chain` primitives we already
depend on:

1. Add a `ContractUtxoTracker`: `SpkTxOutIndex<ContractId>` (or
   `KeychainTxOutIndex<K>` with a contract key type) over its own `TxGraph`,
   sharing the wallet's `LocalChain` view. Index each contract's funding SPK at
   offer/accept time (the script is derivable from the offer+accept messages —
   same derivation `contract/splice.rs` already uses).
2. Extend `sync` to run a second, targeted `SyncRequest` carrying the funding
   SPKs and outpoints. Esplora resolves outpoint spend status, so we learn both
   confirmation of the funding tx and any spend of the funding output
   (CET, refund, or counterparty close) in the same round trip.
3. Report `contract_confirmed` / `contract_pending` in `crate::Balance` from
   `TxGraph::balance()` over the tracker's outpoints — chain truth instead of
   collateral math. Keep PnL from contract state.
4. Persist the tracker with the serde `bdk_chain` changesets
   (`tx_graph::ChangeSet`, indexer changeset) through the `Storage` trait,
   next to the wallet ChangeSet.
5. Surface `contract_utxos()` on the wallet/DDK API for consumers (ddk-node,
   FFI) so a UI can list locked collateral per contract.

This also gives spend detection for the funding output — the input the manager
needs to notice a counterparty unilateral close without polling
`get_transaction_confirmations` per contract.

## Phase 4 — Wallet events and real fee estimation (~1-2 days)

1. Switch `apply_update` → `Wallet::apply_update_events` and forward
   `WalletEvent`s (`TxConfirmed`, `TxReplaced { conflicts }`, `TxDropped`,
   `TxUnconfirmed`) on a broadcast channel. Trigger the manager's
   `PeriodicCheck` when a relevant event fires instead of only on a timer;
   consumers get reorg-aware confirmation notifications for free.
2. Replace hardcoded fees. `fee_estimator()` (`mod.rs:869-897`) returns
   constants, and `EsploraClient`'s `FeeEstimator` returns 1 sat/kw
   (`esplora.rs:195-199`). Fetch `get_fee_estimates` from esplora on each sync,
   cache into the `AtomicU32` map per `ConfirmationTarget`, and keep the
   constants only as a floor/fallback.
3. Deduplicate `SendToAddress`/`SendAll` into one build-sign-broadcast helper.

## Phase 5 — Labels and coin control (nice to have, ~2 days)

1. BIP-329 labels via the `bip329` crate (the BDK-sanctioned approach —
   bdk_wallet has no native support, issue #168). Key labels by
   `Txid`/`OutPoint`/`Address`, store through the `Storage` trait.
2. Auto-label on contract events: funding tx and funding outpoint get the
   contract id; CET/refund txs get the outcome. Export/import BIP-329 JSONL.
3. Coin-control send API: caller-selected UTXOs (`add_utxo` +
   `manually_selected_only`), an `unspendable` exclusion list, and
   `exclude_below_confirmations`.
4. Fee bumping via `Wallet::build_fee_bump` for stuck sends
   (RBF is already on by default in BDK 3.x).
5. Make `MIN_CHANGE_SIZE` (25 000 sats) a builder option.

---

## Splice signing: keep the PSBT boundary, do not chase descriptors

Contract funding keys use a sha256-hardening step (`contract/keys.rs:192-208`).
They are **not** BIP32-derivable, so they can never live in a descriptor
keychain — "add contract keys to the keychain" is not expressible in BDK, and
that is fine:

- Our signing design is already PSBT-first (`contract/psbt.rs`), which is the
  exact direction BDK is moving (`Wallet::sign_psbt` in 3.2, `bdk_tx`
  `Finalizer` in 4.0). The BDK `signer` module (including 3.1's
  `sign_with_signers`) is deprecated for removal in 4.0 — do not build on it.
- For splices, keep `ContractKeyProvider` + `DlcInputSigningKey`. The Phase 3
  tracker makes the spliced funding UTXO visible and lockable, which is the
  actual gap.
- If wallet UTXOs fund a splice alongside a DLC input, `add_foreign_utxo` (fixed
  in 3.1.0) is the supported way to let the BDK wallet fund a transaction it
  cannot fully sign.

## Watch list (re-evaluate at Wallet 4.0)

- **Multi-keychain `KeyRing`** — bdk_wallet PR #524, very active (updated
  2026-08-13). When released, the Phase 3 tracker can fold into the wallet as a
  real contract keychain.
- **`bdk_tx` / `Wallet::create_psbt`** — unstable on master; a better fit for
  multi-party funding transactions than `TxBuilder` once stable.
- **Balance trust classification** (bdk_wallet #431) — will fix
  trusted/untrusted pending semantics that `ddk.rs::balance()` currently maps
  to `change_unconfirmed`/`foreign_unconfirmed`.
- **bdk_kyoto 0.17** (compact block filters) — shipped in bdk-ffi mobile
  bindings, but pre-1.0, and filters match scripts, not outpoints — weaker fit
  for contract-outpoint watching than esplora.

## Test additions (each phase lands with its tests)

- Restore-from-seed with used change addresses (catches the Phase 1 scan bug).
- Receive-to-mempool visibility without a new block; eviction after RBF.
- Two concurrent offers must not select overlapping UTXOs; locks survive a
  restart.
- Contract funding output: confirmation detection, spend detection (CET and
  refund), and balance split (`spendable`/`reserved`/`contract_*`).
