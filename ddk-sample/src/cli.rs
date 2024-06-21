use std::net::SocketAddr;
use std::str::FromStr;

use clap::{Parser, Subcommand};
// use ddk::dlc_manager::contract::contract_input::ContractInput;
use ddk::bdk::bitcoin::Address;
use ddk::DdkTransport;
use ddk::dlc_manager::Storage;
use ddk::dlc_messages::{Message, OfferDlc};
// use ddk::dlc_messages::contract_msgs::ContractInfo;
// use ddk::dlc_messages::oracle_msgs::OracleAnnouncement;

use crate::ApplicationDdk;
/// CLI defines the overall command-line interface.
#[derive(Parser, Debug)]
#[command(name = "DLC Dev Kit Sample")]
#[command(about = "Test out DDK", long_about = None)]
pub struct DdkCli {
    #[command(subcommand)]
    pub command: Option<Commands>,
    #[arg(short, long)]
    port: Option<u16>
}

/// Commands define the possible subcommands.
#[derive(Subcommand, Debug)]
pub enum Commands {
    Info,
    Listpeers,
    Addpeer {
        #[arg(long)]
        pubkey: String,
        #[arg(long)]
        host: String,
    },
    Wallet {
        #[command(subcommand)]
        wallet_subcommand: WalletCommand,
    },
    Listcontracts,
    Listoffers,
    Closedcontracts,
    Contract {
        #[arg(short, long)]
        contract_id: String,
        // could be a file
    },
    Accept {
        #[arg(short, long)]
        offer: String
    },
    SendOffer {
        #[arg(short, long)]
        contract_input: String,
        #[arg(short, long)]
        counterparty: String,
        #[arg(short, long)]
        announcement: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum WalletCommand {
    Balance,
    NewAddress,
    Send {
        address: String,
        amount: u64,
        sat_vbyte: u64
    }
}

pub fn match_ddk_command(command: Commands, ddk: &ApplicationDdk) -> anyhow::Result<()> {
    match command {
        Commands::Info => {
            println!("Node id: {}", ddk.transport.node_id.to_string());
            println!("Network: {}", ddk.network())
        }
        Commands::Listpeers => {
            let peers = ddk.transport.peer_manager().get_peer_node_ids();
            if peers.is_empty() {
                return Ok(println!("No peers"));
            }
            peers.iter().for_each(|peer| {
                println!("Peer: {}\t\tHost: {:?}", peer.0, peer.1);
            })
        }
        Commands::Addpeer { pubkey, host } => {
            let peer_manager = ddk.transport.peer_manager().clone();
            let pubkey = ddk::bdk::bitcoin::secp256k1::PublicKey::from_str(&pubkey)?;
            let host = SocketAddr::from_str(&host)?;
            tokio::runtime::Builder::new_current_thread().build().unwrap().spawn(async move {
                let connect = lightning_net_tokio::connect_outbound(peer_manager, pubkey, host).await;
                if connect.is_some() {
                    println!("Connected?");
                }
            });
             
            println!("Add: {:?} {:?}", pubkey, host);
        }
        Commands::Listcontracts => {
            let contracts = ddk.storage.get_contracts()?;
            contracts.iter().for_each(|contract| {
                println!("{:?}\n", contract)
            })
        }
        Commands::Listoffers => {
            let offers = ddk.storage.get_offered_channels()?;
            offers.iter().for_each(|offer| {
                println!("{:?}", offer)
            })
        }
        Commands::Closedcontracts => {
            let contracts = ddk.storage.get_preclosed_contracts()?;
            contracts.iter().for_each(|_contract| {
                println!("Closed contracts")
            })
        }
        Commands::Accept { offer } => {
            let offer = serde_json::from_str::<OfferDlc>(&offer)?;
            let (contract_id, counterparty, accept_offer) = ddk.manager.lock().unwrap().accept_contract_offer(&offer.temporary_contract_id)?;
            ddk.transport.send_message(counterparty, Message::Accept(accept_offer));
            println!("Accepted offer: {:?}", contract_id);
        }
        Commands::SendOffer { .. } => {
            // let counterparty = ddk::bdk::bitcoin::secp256k1::PublicKey::from_str(&counterparty)?;
            // let announcement = serde_json::from_str::<OracleAnnouncement>(&announcement)?;
            // let contract_input = serde_json::from_str::<ContractInput>(&contract_input)?;
            // let (contract_id, counterparty, offer) = ddk.manager.lock().unwrap().send_offer_with_announcements(&contract_input, counterparty, vec![vec![announcement]])?;
            // ddk.transport.send_message(counterparty, Message::Offer(offer));
            // println!("Offered Contract: {:?}", contract_id);
            // println!("{:?}", offer);
        }
        Commands::Contract { contract_id } => {
            let mut buf = [0u8; 32];
            buf.copy_from_slice(contract_id.as_bytes());
            let contract = ddk.storage.get_contract(&buf)?;
            println!("{:?}", contract);
        }
        Commands::Wallet { wallet_subcommand } => {
            match wallet_subcommand {
                WalletCommand::Balance => {
                    let balance = ddk.wallet.get_balance()?;
                    println!("{:?}", balance)
                },
                WalletCommand::NewAddress => {
                    let address = ddk.wallet.new_external_address()?;
                    println!("{}", address)
                }
                WalletCommand::Send { address, amount, sat_vbyte } => {
                    let address = Address::from_str(&address)?.assume_checked();
                    let send = ddk.wallet.send_to_address(address, amount, sat_vbyte)?;
                    println!("Sent transaction: {:?}", send)
                }
            }
        }
    }
    Ok(())
}
