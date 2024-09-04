use core::panic;

use clap::{Parser, Subcommand};
use ddk::bdk::bitcoin::{Address, Transaction};
use ddk::bdk::LocalOutput;
use ddk::dlc::{EnumerationPayout, Payout};
use ddk::dlc_manager::contract::contract_input::ContractInput;
use ddk::dlc_manager::contract::offered_contract::OfferedContract;
use ddk::dlc_messages::{AcceptDlc, OfferDlc};
use ddk_node::ddkrpc::ddk_rpc_client::DdkRpcClient;
use ddk_node::ddkrpc::{
    AcceptOfferRequest, ConnectRequest, GetWalletTransactionsRequest, InfoRequest, ListOffersRequest, ListPeersRequest, ListUtxosRequest, NewAddressRequest, SendOfferRequest, WalletBalanceRequest
};
use inquire::{Select, Text};

#[derive(Debug, Clone, Parser)]
#[clap(name = "ddk")]
#[command(about = "A CLI tool for DDK", version = "1.0")]
struct DdkCliArgs {
    #[arg(short, long)]
    #[arg(help = "DDK gRPC server to connect to.")]
    #[arg(default_value = "http://127.0.0.1:3030")]
    pub server: String,
    #[clap(subcommand)]
    pub command: CliCommand,
}

#[derive(Debug, Clone, Subcommand)]
enum CliCommand {
    // Gets information about the DDK instance
    Info,
    // Pass a contract input to send an offer
    OfferContract(Offer),
    // Retrieve the offers that ddk-node has received.
    Offers,
    // Accept a DLC offer with the contract id string.
    AcceptOffer(Accept),
    // Wallet commands
    #[clap(subcommand)]
    Wallet(WalletCommand),
    /// Get the peers connected to the node.
    Peers,
    /// Connect to another DDK node.
    Connect {
        #[arg(help = "The counter party to connect to. <PUBKEY>@<HOST>")]
        connect_string: String
    },
}

#[derive(Parser, Clone, Debug)]
struct Offer {
    #[arg(help = "Path to a contract input file. Eventually to be a repl asking contract params")]
    #[arg(short = 'f', long = "file")]
    pub contract_input_file: Option<String>,
    #[arg(help = "The contract counterparty to send to.")]
    pub counter_party: String,
}

#[derive(Clone, Debug, Subcommand)]
enum WalletCommand {
    #[command(about = "Get the wallet balance.")]
    Balance,
    #[command(about = "Generate a new, unused address from the wallet.")]
    NewAddress,
    #[command(about = "Get the wallet transactions.")]
    Transactions,
    #[command(about = "Get the wallet utxos.")]
    Utxos,
}

#[derive(Parser, Clone, Debug)]
struct Accept {
    // The contract id string to accept.
    pub contract_id: String,
}

#[derive(Parser, Clone, Debug)]
struct Connect {
    #[arg(help = "The public key to connect to.")]
    pub pubkey: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = DdkCliArgs::parse();

    let mut client = DdkRpcClient::connect(args.server).await?;

    match args.command {
        CliCommand::Info => {
            let info = client.info(InfoRequest::default()).await?.into_inner();
            println!("{:?}", info);
        }
        CliCommand::OfferContract(arg) => {
            let contract_input = if let Some(file) = arg.contract_input_file {
                let contract_string = std::fs::read_to_string(file)?;
                serde_json::from_str::<ContractInput>(&contract_string)?
            } else {
                let contract_type = Select::new("Select type of contract.", vec!["enum", "numerical"]).prompt()?;
                match contract_type {
                    "numerical" => {
                        let offer_collateral: u64 = Text::new("Collateral from you (sats):").prompt()?.parse()?;
                        let accept_collateral: u64 = Text::new("Collateral from counterparty (sats):").prompt()?.parse()?;
                        let fee_rate: u64 = Text::new("Fee rate (sats/vbyte):").prompt()?.parse()?;
                        let min_price: u64 = Text::new("Minimum Bitcoin price:").prompt()?.parse()?;
                        let max_price: u64 = Text::new("Maximum Bitcoin price:").prompt()?.parse()?;
                        let num_steps: u64 = Text::new("Number of rounding steps:").prompt()?.parse()?;
                        let oracle_pubkey = Text::new("Oracle public key:").prompt()?;
                        let event_id = Text::new("Oracle event id:").prompt()?;
                        ddk_payouts::create_contract_input(min_price, max_price, num_steps, offer_collateral, accept_collateral, fee_rate, oracle_pubkey, event_id)
                    }
                    "enum" => {
                        let offer_collateral: u64 = Text::new("Collateral from you (sats):").prompt()?.parse()?;
                        let accept_collateral: u64 = Text::new("Collateral from counterparty (sats):").prompt()?.parse()?;
                        let num_outcomes: usize = Text::new("Number of outcomes:").prompt()?.parse()?;
                        let mut outcome_payouts = Vec::with_capacity(num_outcomes);
                        for _ in 0..num_outcomes {
                            let outcome = Text::new("Outcome:").prompt()?;
                            let offer: u64 = Text::new("Payout: ").prompt()?.parse()?;
                            let accept: u64 = Text::new("Counterparty payout:").prompt()?.parse()?;
                            let outcome_payout = EnumerationPayout {
                                outcome,
                                payout: Payout {
                                    offer,
                                    accept,
                                }
                            };
                            outcome_payouts.push(outcome_payout)
                        }
                        let fee_rate: u64 = Text::new("Fee rate (sats/vbyte):").prompt()?.parse()?; 
                        let oracle_pubkey = Text::new("Oracle public key:").prompt()?;
                        let event_id = Text::new("Oracle event id:").prompt()?;
                        ddk_payouts::enumeration::create_contract_input(outcome_payouts, offer_collateral, accept_collateral, fee_rate, oracle_pubkey, event_id)
                    }
                    _ => panic!("Invalid contract type.")
                }
            };

            let contract_input = serde_json::to_vec(&contract_input)?;
            let offer = client.send_offer(SendOfferRequest { contract_input, counter_party: arg.counter_party}).await?.into_inner();
            let offer_dlc = serde_json::from_slice::<OfferDlc>(&offer.offer_dlc)?;
            println!("{:?}", offer_dlc);
        }
        CliCommand::Offers => {
            let offers_request = client.list_offers(ListOffersRequest {}).await?.into_inner();
            let offers: Vec<OfferedContract> = offers_request
                .offers
                .iter()
                .map(|offer| serde_json::from_slice(offer).unwrap())
                .collect();
            let offer_ids = offers.iter().map(|o| hex::encode(&o.id)).collect::<Vec<String>>();
    
            let offer = inquire::Select::new("Select offer to view.", offer_ids).prompt()?;

            let mut offer_bytes = [0u8;32];
            let chosen_offer = hex::decode(&offer)?;
            offer_bytes.copy_from_slice(&chosen_offer);
            let offer = offers.iter().find(|o| o.id == offer_bytes);
            if let Some(o) = offer {
                print!("{}", serde_json::to_string_pretty(&o).unwrap())
            }
        }
        CliCommand::AcceptOffer(accept) => {
            let accept = client
                .accept_offer(AcceptOfferRequest {
                    contract_id: accept.contract_id,
                })
                .await?
                .into_inner();
            println!("Contract {} accepted with {}", accept.contract_id, accept.counter_party);
            let accept_dlc = serde_json::from_slice::<AcceptDlc>(&accept.accept_dlc)?;
            println!("{:?}", accept_dlc)
        }
        CliCommand::Wallet(wallet) => match wallet {
            WalletCommand::Balance => {
                let balance = client
                    .wallet_balance(WalletBalanceRequest::default())
                    .await?
                    .into_inner();
                println!("Balance: {:?}", balance);
            }
            WalletCommand::NewAddress => {
                let address = client
                    .new_address(NewAddressRequest::default())
                    .await?
                    .into_inner();
                println!("{:?}", address)
            }
            WalletCommand::Transactions => {
                let transactions = client
                    .get_wallet_transactions(GetWalletTransactionsRequest::default())
                    .await?
                    .into_inner();
                for tx in transactions.transactions {
                    let transaction: Transaction = serde_json::from_slice(&tx.transaction)?;
                    println!("TxId: {:?}", transaction.txid().to_string());
                    for output in transaction.output {
                        println!(
                            "\t\tValue: {:?}\tAddress: {:?}",
                            output.value,
                            Address::from_script(
                                &output.script_pubkey,
                                ddk::bdk::bitcoin::Network::Regtest
                            )
                        )
                    }
                }
            }
            WalletCommand::Utxos => {
                let utxos = client
                    .list_utxos(ListUtxosRequest::default())
                    .await?
                    .into_inner();
                for utxo in utxos.utxos {
                    let utxo: LocalOutput = serde_json::from_slice(&utxo)?;
                    println!(
                        "TxId: {:?} Index: {:?}",
                        utxo.outpoint.txid, utxo.outpoint.vout
                    );
                    println!(
                        "\t\tAddress: {:?}",
                        Address::from_script(
                            &utxo.txout.script_pubkey,
                            ddk::bdk::bitcoin::Network::Regtest
                        )
                    );
                    println!("\t\tValue: {:?}", utxo.txout.value);
                }
            }
        },
        CliCommand::Peers => {
            let peers = client.list_peers(ListPeersRequest::default()).await?.into_inner();
            peers.peers.iter().for_each(|peer| println!("{}\t{}", peer.pubkey, peer.host))
        }
        CliCommand::Connect { connect_string } => {
            let parts = connect_string.split("@").collect::<Vec<&str>>();
            client.connect_peer(ConnectRequest { pubkey: parts[0].to_string(), host: parts[1].to_string() }).await?;
            println!("Connected")
        }
    }

    Ok(())
}
