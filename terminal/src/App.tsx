import { useEffect, useState } from "react";
import { getPubkeys, listPeers, newAddress } from "./ernest";
import "./App.css";

function App() {
  const [address, setAddress] = useState<string>("")
  const [pubkeys, setPubkeys] = useState<{ node_id: string, bitcoin: string }>()
  const [peers, setPeers] = useState<string[]>([])

  const getNewAddress = async () => {
    const addr = await newAddress()
    setAddress(addr)
  }

  const getWalletPubkeys = async () => {
    const pubkeys = await getPubkeys()
    setPubkeys(pubkeys)
  }

  const getPeers = async () => {
    console.log("gettings peers")
    const peers = await listPeers()
    setPeers(peers)
  }

  useEffect(() => {
    getNewAddress()
    getWalletPubkeys()
  }, [])

  return (
    <div className="container">
      <h1>Ernest Money</h1>
      <p>{address}</p>
      <h3>LDK Node Id</h3>
      <p>{pubkeys?.node_id}</p>
      <h3>Bitcoin Pubkey</h3>
      <p>{pubkeys?.bitcoin}</p>
      <h3>Peers {peers?.length}</h3>
      <button onClick={() => getPeers()}>List Peers</button>
      {peers && peers.map(p => <p key={p}>{p}</p>)}
    </div>
  );
}

export default App;
