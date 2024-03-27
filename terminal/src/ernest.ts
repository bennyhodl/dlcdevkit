import { invoke } from "@tauri-apps/api/tauri";

export async function getPubkeys(): Promise<{bitcoin: string, node_id: string}> {
  const keys = await invoke<{bitcoin: string, node_id: string}>("get_pubkeys");

  return { bitcoin: keys.bitcoin, node_id: keys.node_id }
}

export async function newAddress(): Promise<string> {
  return await invoke("new_address")
}

export async function listPeers(): Promise<string[]> {
  return await invoke("list_peers")
}
