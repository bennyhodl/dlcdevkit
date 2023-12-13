import { invoke } from "@tauri-apps/api/tauri";

export async function getPubkeys(): Promise<{nostr: string, bitcoin: string}> {
  const keys = await invoke<{nostr: string, bitcoin: string}>("get_pubkeys");

  return { nostr: keys.nostr, bitcoin: keys.bitcoin }
}

export async function newAddress(): Promise<string> {
  return await invoke("new_address")
}