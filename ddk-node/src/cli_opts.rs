use clap::{Parser, Subcommand};

#[derive(Debug, Clone, Subcommand)]
pub enum CliCommand {
    /// Gets information about the DDK instance
    Info,
    /// Pass a contract input to send an offer
    OfferContract(Offer),
    /// Retrieve the offers that ddk-node has received.
    Offers,
    /// Accept a DLC offer with the contract id string.
    AcceptOffer(Accept),
    /// List contracts.
    Contracts,
    #[command(about = "Get the wallet balance.")]
    Balance,
    /// Wallet commands
    #[clap(subcommand)]
    Wallet(WalletCommand),
    /// Interface with the oracle
    #[clap(subcommand)]
    Oracle(OracleCommand),
    /// Get the peers connected to the node.
    Peers,
    /// Connect to another DDK node.
    Connect {
        #[arg(help = "The counter party to connect to. <PUBKEY>@<HOST>")]
        connect_string: String,
    },
    /// Sync the wallet and contracts.
    #[command(about = "Sync the wallet and contracts.")]
    Sync,
}

#[derive(Parser, Clone, Debug)]
pub struct Offer {
    #[arg(help = "Generate a contract automatically with peer.")]
    #[arg(short = 'g', long = "generate", default_value = "false")]
    pub generate: bool,
    #[arg(help = "The contract counterparty to send to.")]
    pub counter_party: String,
}

#[derive(Clone, Debug, Subcommand)]
pub enum WalletCommand {
    #[command(about = "Generate a new, unused address from the wallet.")]
    NewAddress,
    #[command(about = "Get the wallet transactions.")]
    Transactions,
    #[command(about = "Get the wallet utxos.")]
    Utxos,
    #[command(about = "Send a Bitcoin amount to an address")]
    Send {
        /// Address to send to.
        address: String,
        /// Amount in sats to send to.
        amount: u64,
        /// Fee rate in sats/vbyte
        fee_rate: u64,
    },
    #[command(about = "Sync the on-chain wallet.")]
    Sync,
}

#[derive(Clone, Debug, Subcommand)]
pub enum OracleCommand {
    #[command(about = "Get all known oracle announcements.")]
    Announcements {
        #[arg(help = "The announcement id to get.")]
        event_id: String,
    },
    #[command(about = "Create an enum oracle event.")]
    CreateEnum {
        #[arg(help = "The maturity of the event.")]
        maturity: u32,
        #[arg(help = "The outcomes of the event. Separate by spaces.")]
        outcomes: Vec<String>,
    },
    #[command(about = "Create a numeric oracle event.")]
    CreateNumeric {
        #[arg(help = "The maturity of the event.")]
        maturity: u32,
        #[arg(help = "Number of digits for the numeric event.")]
        nb_digits: u32,
    },
    #[command(about = "Sign an oracle announcement.")]
    Sign {
        #[arg(
            long,
            help = "Specify if the event is enum.",
            conflicts_with = "numeric"
        )]
        r#enum: bool,
        #[arg(
            long,
            help = "Specify if the event is numeric.",
            conflicts_with = "enum"
        )]
        numeric: bool,
        #[arg(long, help = "The outcome to sign.")]
        outcome: String,
        #[arg(long, help = "The event id to sign.")]
        event_id: String,
    },
}

#[derive(Parser, Clone, Debug)]
pub struct Accept {
    // The contract id string to accept.
    pub contract_id: String,
}

#[derive(Parser, Clone, Debug)]
pub struct Connect {
    #[arg(help = "The public key to connect to.")]
    pub pubkey: String,
}
