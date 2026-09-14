# Disjoint-union contracts

A disjoint union is **OR between events**, with a separate oracle threshold
inside each event. Any branch can spend the same funding output; only one of
those competing spends can confirm. It does not require all events to happen,
combine unrelated facts into a threshold vote, or enforce priority between
repayment, liquidation and maturity.

The participants know the whole contract. Each oracle only needs its event's
announcement and observation; it need not receive the other events, payouts,
funding address or loan identity. Splitting events enables that separation, but
operational privacy still depends on what the application sends each oracle.

## What was already implemented

- `ddk-messages`: `ContractInfo::DisjointContractInfo` carries multiple
  `ContractInfoInner` entries, each with its own descriptor and oracle info.
- `ddk-manager`: `ContractInput.contract_infos` supports a separate event ID,
  descriptor and oracle set per branch. Conversion already preserves these.
- Enum and numerical descriptors already generate payouts, CET adaptor points,
  and threshold-oracle signatures. A new descriptor variant is not needed.
- The stateless `ddk::contract` lifecycle already builds concatenated CET and
  adaptor-signature arrays, with separate CET ranges for each branch.

See the upstream [contract negotiation specification](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Protocol.md)
and [threshold-oracle construction](https://github.com/discreetlogcontracts/dlcspecs/blob/master/MultiOracle.md).

## Gaps fixed here

| Path | Problem | Correction |
| --- | --- | --- |
| Both transaction builders | Later branches used locktime zero | Preserve the first branch/offer CET locktime |
| Manager signing | Every branch signed against the start of the combined CET array | Give each branch its own CET slice |
| Manager sign verification | Every branch verified against the start of that array | Use the corresponding CET slice |
| Manager settlement | Branch-local CET indexes and enum adaptor indexes were used as global indexes | Reconstruct both offsets from preceding branches; numerical trie adaptor indexes already include their offset |
| Stateless settlement | A shared enum label could select an earlier event and immediately fail validation | Continue to later branches; return the validation error if none accepts the attestations |
| Shared outcome lookup | Asynchronous response order could select the wrong oracle combination | Sort by announcement index before looking up the outcome |
| Manager automatic settlement | Enough responses, including empty responses or disagreement, could block later branches | Select a branch only when it has a usable outcome and signature set |
| Manager maturity polling | Filtering then enumerating renumbered the remaining oracles | Enumerate before filtering |

The manager's subsequent-branch verification also now uses the supplied funding
script and counterparty adaptor key consistently with the first branch. These
are overridden by the channel caller; ordinary contracts use their funding
script and funding public key.

## Baseball demonstration

The shared fixture is `testenv/src/dlc/baseball.rs`. It creates three independent
oracle groups, each **2-of-3**, with different keys and event IDs:

| Branch | Oracle fact | Outcome | Offer payout when true / false |
| --- | --- | --- | --- |
| 0 (enum) | Hitter has exactly two hits | `yes` / `no` | 90% / 10% |
| 1 (numerical) | Hitter's at-bat count | Unsigned 4-bit number, 0–15; predicate ≥5 | 80% / 20% |
| 2 (enum) | Hitter's team wins | `yes` / `no` | 70% / 30% |

The accepter gets the remainder. Distinct payouts make a wrong branch index
observable. The two enums deliberately share outcome labels. Only oracle
indexes 1 and 2 attest; oracle 0 matures later, and the other two events do not
attest at all. This demonstrates independent settlement, not an AND bet.

`ddk/tests/stateless.rs` completes the wire-message lifecycle and checks both
parties can settle every branch. It round-trips offer/accept/sign messages,
verifies both resulting ECDSA signatures, the funding outpoint, payouts and
locktime, rejects insufficient attestations, and supplies attestations in reverse
order. Cases cover both enum outcomes and at-bats 4, 5 and 15.

`ddk-manager/tests/manager_execution_tests.rs` funds real regtest contracts,
settles all seven cases manually from either party, and settles each successful
branch automatically from either party. It checks payouts, locktime, rejects
missing/insufficient/duplicate/out-of-range attestations, mines the CET, and
checks both managers reach `Closed`.

Run:

```sh
cargo test -p ddk --no-default-features --features manager --test stateless baseball_disjoint
cargo test -p ddk-manager --features use-serde --test manager_execution_tests baseball_disjoint -- --ignored --test-threads=1 --nocapture
```

The second command downloads/starts managed Bitcoin Core and electrs backends;
no pre-existing node is required. It runs 20 separate funded-contract scenarios.

## Scope and remaining work

- `OracleInput` still uses one event ID within a threshold group. Distinct IDs
  **between branches** work already; distinct IDs per oracle inside one group
  would require a separate input API change.
- The baseball proof covers ordinary on-chain contracts in the stateless and
  manager APIs. Channel unilateral settlement still uses branch-local indexes
  against combined arrays in `finalize_unilateral_close_settled_channel`; that
  path needs the same offset treatment and separate channel execution tests.
- The singular `Contract::get_oracle_announcement` helper and PostgreSQL's
  `contract_metadata` announcement/key columns expose only the first oracle of
  the first branch. The serialized contract retains all branches, but consumers
  using that metadata need a multi-event view for discovery/display.
- Lygos must choose branch payouts and event timing deliberately. An enum's
  negative outcome also has a CET; “not repaid yet” must not be published as a
  final event if it should leave the loan open. If two independent branches
  attest, the contract itself does not give one priority.
- The locktime correction changes later-branch transaction IDs for newly built
  multi-event contracts. Existing persisted manager contracts keep their stored
  CETs. Stateless callers reconstruct CETs from messages, so historical
  multi-event messages signed using zero locktime on later branches need legacy
  handling; this change does not add a versioned legacy reconstruction mode.

## Additional verification

- Both baseball manager tests passed: 20 funded-contract scenarios (14 manual,
  6 automatic). The automatic test also passed separately with
  `NB_CONFIRMATIONS=6`, matching CI.
- `cargo test -p ddk --no-default-features --features manager --test stateless`:
  33 tests passed.
- `cargo test -p ddk-manager --features use-serde --lib`: 56 tests passed.
- Existing `enum_and_numerical_with_diff_3_of_5_manual_test`: passed on regtest.
- `cargo check -p ddk-manager --no-default-features --features std`: passed.
- Targeted Clippy (`ddk`, `ddk-manager`, library targets, `--no-deps`,
  `--no-default-features --features manager,use-serde`, `-D warnings`): passed.
  Including dependency lints hits a pre-existing `explicit_counter_loop` warning
  in `kormir/src/storage.rs:93` with the installed Rust 1.98 toolchain.
- Changed Rust files pass rustfmt; `git diff --check` is clean.
- CodeRabbit CLI is installed but signed out; review was performed locally.
