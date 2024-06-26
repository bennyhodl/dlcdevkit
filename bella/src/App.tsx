import { useEffect, useState } from "react";
import { getPubkeys, listPeers, newAddress, listContracts, listOffers, acceptOffer, getBalance } from "./bella";
import "./App.css";

function App() {
  const [address, setAddress] = useState<string>("")
  const [pubkeys, setPubkeys] = useState<{ node_id: string, bitcoin: string }>()
  const [peers, setPeers] = useState<string[]>([])
  const [offers, setOffers] = useState<any[]>([])
  const [balance, setBalance] = useState<any>({})

  const getWalletBalance = async () => {
    const balance = await getBalance()
    setBalance(balance)
  }

  const getNewAddress = async () => {
    const addr = await newAddress()
    setAddress(addr)
  }

  const getWalletPubkeys = async () => {
    const pubkeys = await getPubkeys()
    setPubkeys(pubkeys)
  }

  const getOffers = async () => {
    const offers = await listOffers()
    setOffers(offers)
  }

  const getPeers = async () => {
    const peers = await listPeers()
    setPeers(peers)
  }

  useEffect(() => {
    getNewAddress()
    getWalletPubkeys()
    getWalletBalance()
  }, [])

  return (
    <div className="container">
      <h1>Bella Money</h1>
      <p>{address}</p>
      <h1>Balance</h1>
      <p>{JSON.stringify(balance)}</p>
      <button onClick={() => getWalletBalance()}>Get Balance</button>
      <h3>LDK Node Id</h3>
      <p>{pubkeys?.node_id}</p>
      <h3>Bitcoin Pubkey</h3>
      <p>{pubkeys?.bitcoin}</p>
      <h3>Peers {peers?.length}</h3>
      <button onClick={() => getPeers()}>List Peers</button>
      {peers && peers.map(p => <p key={p}>{p}</p>)}
      <h3>Offers {offers?.length}</h3>
      <button onClick={() => getOffers()}>List Offers</button>
      {offers && offers.map(o => {
        return (
          <div style={{ display: "flex", flexDirection: "row", justifyContent: "space-around", padding: "10px 0" }}>
            <p>{o.id}</p>
            <button onClick={async () => await acceptOffer(o.id)}>Accept DLC</button>
          </div>
        )
      })}
      <button onClick={() => listContracts()}>List Contracts</button>
    </div>
  );
}

export default App;
