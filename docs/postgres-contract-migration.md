# Postgres contract migration

Releases up to 2.0 stored each contract as one opaque blob in `contract_data`,
with a copy of a few fields in `contract_metadata`. From this release the
Postgres store keeps one row per contract in `dlc_contracts`, with the offer,
accept, and sign messages in their own columns and the manager-only state in
typed columns. See issue #190 for why.

## What happens on upgrade

1. The schema migration `0010_dlc_contracts` creates the new table. It does
   not touch the old tables.
2. When the store opens with migrations on, which is how `ddk-node` and
   `Builder` open it, it moves every contract from the old tables to the new
   table. One transaction per contract, so a contract is always in exactly one
   place.
3. The store logs a warning at startup, and on every read that hits the old
   tables, while any contract is still in the old layout.
4. Old rows still load. A contract that is read from the old layout moves to
   the new one on its next update.

Nothing is deleted from the old tables except the rows that were moved. The
two old tables are dropped in a later release, after the legacy reader goes.

## Running the migration by hand

Take a backup first:

```sh
pg_dump "$DATABASE_URL" > before-migration.sql
```

Then run the migration and exit:

```sh
ddk-node --postgres-url "$DATABASE_URL" migrate
```

The command prints how many contracts it moved and names every contract it
could not move, with the error. It exits non-zero when any contract is left
behind. It is safe to run again; it only selects what is left.

An application that embeds the store calls the same function:

```rust
let report = store.migrate_legacy_contracts().await?;
if !report.is_complete() {
    for (id, error) in &report.failed {
        eprintln!("{id}: {error}");
    }
}
```

`PostgresStore::count_legacy_contracts` tells how many contracts are left
without moving any.

## The row layout

`format_version` on each row says which layout the row uses. Version 1 is the
blob layout in `contract_data` and never appears in `dlc_contracts`. Version 2
is the columnar layout. A reader that meets a version it does not know
returns an error instead of misreading the row.

The one writer is `ContractRow::from_contract` and the one reader is
`ContractRow::into_contract`, both in `ddk/src/storage/postgres/contract_row.rs`.
Every read path of the store goes through them. Adding a field to a stored
contract is a schema migration plus a change to those two functions.

The TLV streams of the offer, accept, and sign messages travel inside the
stored messages, so they survive on every state without a suffix.
