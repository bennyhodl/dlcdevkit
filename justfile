pack:
  wasm-pack build ./wasm --dev --weak-refs --target web --scope ernest

link:
  wasm-pack build ./wasm --dev --weak-refs --target web --scope ernest && cd wasm/pkg && pnpm link --global
