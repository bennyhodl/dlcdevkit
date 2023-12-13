import { useEffect, useState } from "react";
import { getPubkeys, newAddress } from "./ernest";
import "./App.css";

function App() {
  const [address, setAddress] = useState<string>("")
  const [pubkeys, setPubkeys] = useState<{ nostr: string, bitcoin: string }>()
  const getNewAddress = async () => {
    const addr = await newAddress()
    setAddress(addr)
  }

  const getWalletPubkeys = async () => {
    const pubkeys = await getPubkeys()
    setPubkeys(pubkeys)
  }

  useEffect(() => {
    getNewAddress()
    getWalletPubkeys()
  }, [])

  return (
    <div className="container">
      <h1>Ernest Money</h1>
      <p>{address}</p>
      <h3>Nostr Pubkey</h3>
      <p>{pubkeys?.nostr}</p>
      <h3>Bitcoin Pubkey</h3>
      <p>{pubkeys?.bitcoin}</p>
    </div>
  );
}

export default App;
