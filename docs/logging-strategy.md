# `dlcdevkit` Logging Strategy

## Overview

`dlcdevkit` implements a structured logging approach using Rust's `tracing` crate and Lightning's `Logger` trait to provide comprehensive visibility into contract operations, channel management, and system events.

## Key Components

### Logger Integration
- All major operations accept a `Logger` parameter (`L: Deref` where `L::Target: Logger`)
- Uses Lightning's logging macros (`log_debug!`, `log_info!`) for consistent formatting
- Provides structured logging with contextual information for debugging and monitoring

### Logging Levels

**Debug Level (`log_debug!`)**:
- Contract party parameter details (fund pubkey, change/payout scripts, input amounts, collateral)
- DLC transaction creation details (funding script, output values, CET counts)
- Input signing operations and verification steps
- Channel cleanup and monitoring operations

**Info Level (`log_info!`)**:
- High-level contract lifecycle events (offer created, contract accepted, signed)
- Major transaction milestones (funding signed, CET execution, refund operations)
- Performance metrics (DLC transaction creation timing)

## Contract Lifecycle Logging

### Contract Creation
- **Offer Contract**: Logs party parameters, temporary ID, counterparty details
- **Accept Contract**: Logs acceptance parameters, DLC transaction creation metrics
- **Sign Contract**: Logs verification steps, signature counts, final contract IDs

### Transaction Operations
- **Funding Transactions**: Input population, signature verification, witness creation
- **CET Operations**: Oracle attestation processing, outcome verification
- **Refund Operations**: Refund transaction signing and verification
- **Cooperative Close**: Transaction creation and signature verification

### Channel Management
- **Chain Monitoring**: Transaction and output watching (debug logging removed for noise reduction)
- **Channel Updates**: Contract renewal operations and finalization steps

## Structured Data Logging

Each log entry includes relevant contextual information:
- **Contract IDs**: Both temporary IDs and final contract IDs for traceability
- **Transaction Details**: TXIDs, script pubkeys, output values
- **Cryptographic Data**: Public keys, signature counts, input/output counts
- **Network Data**: Counterparty information, channel IDs

## Performance Monitoring

- **DLC Creation Timing**: Measures CET generation performance
- **Input Processing**: Tracks funding input counts and processing
- **Signature Operations**: Monitors adaptor signature verification and creation

## Configuration

The logging system is configurable through:
- Environment variables for log levels
- Runtime logger configuration via the `Logger` trait implementation
- Integration with external logging frameworks through the Lightning Logger interface

## Usage Examples

```rust
// Contract offer with logging
let (offered_contract, offer_msg) = offer_contract(
    secp,
    &contract_input,
    // ... other parameters
    &logger,  // Logger instance
).await?;

// This will log:
// DEBUG: Created offer contract with offer party params. temp_id=abc123...
```

## Benefits

1. **Debugging**: Detailed trace of contract operations for troubleshooting
2. **Monitoring**: Performance metrics and operational insights
3. **Audit Trail**: Complete record of contract lifecycle events
4. **Integration**: Compatible with external logging and monitoring systems
5. **Structured Data**: Machine-readable log formats for automated processing

## Implementation Notes

- Logging overhead is minimal due to efficient `tracing` implementation
- Debug logs can be disabled in production for performance
- Hex encoding used for cryptographic data display
- Consistent formatting across all components for log aggregation