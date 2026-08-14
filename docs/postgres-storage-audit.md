# Postgres Storage Audit

Audit of the Postgres wallet/contract storage (`ddk/src/storage/postgres/`) against
the code, plus 7 days of dlcd-rs logs from staging and production (2026-08).
The deployed storage code (ddk 1.1.2) is identical to local HEAD, so all
findings apply to both environments.

**Headline findings:**

1. The `block` table grows without bound and the full table is read on every
   process start. Staging read **843,516 rows in up to 53.75 s** — the k8s
   startupProbe kills the pod at 60 s, so staging trends toward a crash-loop.
2. `delete_contract` can never delete a row — it always errors and rolls back.
3. BDK changeset fields `first_seen`, `last_evicted`, and `spk_cache` are
   silently dropped on persist.
4. Production logs show zero Postgres errors and zero slow statements in 7
   days; staging shows ~109 slow-statement warnings/week. All staging slowness
   traces to the code patterns below.

---

## Incorrect queries

All in `ddk/src/storage/postgres/mod.rs` unless noted. Line numbers are at the
time of the audit.

1. **`delete_contract` always fails** (lines 420-430). It runs
   `query_as::<_, ContractMetadata>("DELETE FROM contract_metadata WHERE id = $1")`
   with `fetch_one`. A `DELETE` without `RETURNING` yields zero rows, so
   `fetch_one` returns `RowNotFound` and the transaction rolls back. Nothing is
   ever deleted. dlcd-rs `repair_contract_state` (`server.rs:605`) depends on
   this to evict phantom contracts and will always fail that step.
   *Fix: use `.execute()` and check `rows_affected`.*

2. **`block` table schema contradicts BDK's data model** (migration
   `0002_bdk_wallet.up.sql`; write at lines 955-976, read at 933-945). BDK's
   `local_chain::ChangeSet` is a map `height → Option<hash>`. The PK is
   `(wallet_name, hash)`, so a reorg inserts a second row at the same height
   and the stale row stays. The read builds a `BTreeMap<height, hash>` with no
   `ORDER BY`, so the orphaned hash can win and the wallet resurrects a
   reorged-out block. The reorg delete path
   (`DELETE FROM block WHERE wallet_name = $1 AND height = $2`) can violate the
   `anchor_tx → block` FK, which rolls back the whole persist and poisons every
   retry. *Fix: PK `(wallet_name, height)` with upsert-on-conflict; decouple
   `anchor_tx` from `block` (anchors carry their own block hash/height in BDK's
   model — an anchor may reference a block no longer in the local chain).*

3. **`update_last_revealed` is not monotonic** (lines 773-789). Plain
   `UPDATE ... SET last_revealed = $1`. BDK's merge rule takes the maximum. A
   stale write regresses the derivation index; after a restart the wallet
   re-reveals used addresses (address reuse).
   *Fix: `SET last_revealed = GREATEST(last_revealed, $1)`.*

4. **Lossy ChangeSet serialization** (`write()` lines 233-292, `read()` lines
   156-189). `tx_graph::ChangeSet::first_seen` and `last_evicted`, and the
   indexer `spk_cache`, are neither persisted nor read, and `persist_async`
   clears the staged changeset on success — the data is permanently lost.
   Losing `last_evicted` can resurrect an RBF-replaced or evicted funding
   transaction as unconfirmed after a restart.

5. **`last_seen` UPDATE with no upsert** (lines 884-891). If a changeset
   carries `last_seen` for a txid whose row does not exist yet, the update
   affects zero rows and the value is silently dropped.

6. **`update_contract` non-atomic upsert with hardcoded values** (lines
   482-573). SELECT-then-INSERT/UPDATE under READ COMMITTED: two concurrent
   updates for a new id both see "missing" and one dies with a unique
   violation. The INSERT fallback hardcodes `is_offer_party = false` and
   `fee_rate_per_vb = 1`, corrupting recreated metadata rows;
   `get_contract_offers` filters on `is_offer_party = false`, so corrupted rows
   leak into the offers list. *Fix: `INSERT ... ON CONFLICT (id) DO UPDATE`.*

7. **`insert_descriptor` / `insert_network` are plain INSERTs** (lines 742-750,
   762-766). Re-staging either (wallet re-create path) hits a unique violation
   and poisons the whole persist transaction. *Fix: idempotent upserts.*

8. **`last_revealed INTEGER DEFAULT 0`** (migration 0002; read at lines
   211-224). A fresh wallet reads back `Some(0)`; BDK treats that as "index 0
   revealed" and skips the first address. *Fix: default NULL.*

9. **Swallowed persist errors** (`ddk/src/wallet/mod.rs:296,307`).
   `let _ = wallet.persist_async(...)` after revealing an address. A failed
   persist is invisible; combined with finding 3 this enables silent address
   reuse.

Minor: `read()` runs five SELECTs in a READ COMMITTED transaction (each sees
its own snapshot — use REPEATABLE READ), and `changeset_from_row` panics on a
bad network string (`expect`, line 203).

## Optimization opportunities

1. **Unbounded `block` table + full read at startup** (lines 933-945). Staging:
   64 slow-statement warnings in 7 days on
   `SELECT hash, height FROM block WHERE wallet_name = $1`, avg 8.8 s, max
   53.75 s at 843,516 rows (~2,880 new rows/day on mutinynet). The staging
   startupProbe window is 60 s. Production grows ~144 rows/day (~15k total) —
   same trajectory, slower. *Fix: BDK needs only sparse checkpoints plus anchor
   blocks; prune non-anchor rows, or persist only the checkpoint set.*

2. **Periodic check re-reads all contract blobs every 30 s.**
   `get_signed_contracts` / `get_confirmed_contracts` / `get_preclosed_contracts`
   (lines 580-666) each pull full serialized contracts by state. Production
   runs exactly 2,880 cycles/day (30 s cadence; ZMQ not configured) — ~8,640
   blob scans/day. Staging: a `state = 4` scan took up to 5.0 s for 16 rows.
   *Fix: filter maturity in SQL against `contract_metadata`, and/or configure
   ZMQ to drop the cadence to 150 s.*

3. **Contract blobs never compressed.** `is_compressed` is always bound `false`
   (lines 397, 568). Single-row PK lookups on `contract_data` took 1.4-2.0 s in
   staging (TOAST detoast of CET adaptor signature blobs). *Fix: zstd-compress,
   or store adaptor signatures separately.*

4. **Row-per-statement persist loops** (lines 872-921, 955-976). One INSERT per
   tx/txout/anchor/block. Staging: slow `COMMIT` in the `write` span (5 hits,
   max 3.5 s) and slow block INSERTs (4 hits). *Fix: batch with `UNNEST`.*

5. **Duplicate indexes** (migration 0003). `idx_contract_metadata_id` and
   `idx_contract_data_id` duplicate the PK indexes — drop both. `idx_block_height`
   omits `wallet_name`, so the startup read has no ideal index anyway.

6. **`get_contracts` unbounded** (lines 337-349; called from dlcd-rs gRPC
   listing and the contract graph). Fetches and deserializes every contract
   ever created, including closed ones. *Fix: state filters / pagination.*

7. **Pool configuration.** dlcd-rs: `DATABASE_MAX_CONNECTIONS=25` (staging) /
   `50` (production), 1 replica each, sqlx default 30 s acquire timeout.
   Production's 50 is oversized but harmless. Staging logged 12 slow-acquire
   warnings in 7 days — DB pressure, not pool exhaustion.

## Log findings (7 days, source `dlcd-service`)

| Metric | Staging | Production |
|---|---|---|
| Total log rows | ~817k (~116k/day, mostly DEBUG) | ~832k (~119k/day, mostly DEBUG) |
| sqlx slow statements (>1 s) | 109 | 0 |
| Slow pool acquires | 12 | 0 |
| Postgres errors | 0 | 0 |
| `Writing changeset` | ~1,390/day (per mutinynet block) | ~135/day (per mainnet block) |
| `Reading changeset` (process starts) | 3-24/day (pod churn) | ~0/day |
| ERROR lines | 314, all esplora/mutinynet HTTP | 1 (gRPC "Contract not found") |

Slow-statement breakdown (staging):

| Statement | Code location | Count | Avg / Max | Max rows |
|---|---|---|---|---|
| `SELECT hash, height FROM block WHERE wallet_name = $1` | mod.rs:933 | 64 | 8.8 s / 53.75 s | 843,516 |
| `SELECT * FROM contract_data WHERE id = $1` | mod.rs:323 | 13 | 1.64 s / 1.97 s | 1 |
| `SELECT * FROM contract_data WHERE state = 5` | mod.rs:651 | 13 | 1.6 s / 2.68 s | 2 |
| `SELECT * FROM contract_data WHERE state = 4` | mod.rs:629 | 8 | 2.05 s / 5.0 s | 16 |
| `COMMIT` (persist_bdk) | mod.rs:287 | 5 | 2.07 s / 3.51 s | — |
| `INSERT INTO block ...` | mod.rs:958 | 4 | 1.75 s / 1.96 s | — |

The `Checking contract for oracle maturation` DEBUG line dominates production
volume (616,685 rows/7 days) — the visible half of optimization 2.

## Not checked

- Live database state (row counts inferred from sqlx `rows_returned` fields).
- `delete_contract` at runtime — no `RowNotFound` in the 7-day window; the
  defect is confirmed by code inspection only.
- Root cause of staging pod churn (3-24 restarts/day) — k8s events not pulled.
- Query latency below 1 s (sqlx logs filter at `sqlx=info`, >1 s only).
- Testnet4 (out of scope).
