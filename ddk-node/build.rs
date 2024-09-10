fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::path::PathBuf::from("./src/proto");

    let protos = ["ddkrpc.proto"];

    let proto_paths: Vec<_> = protos
        .iter()
        .map(|proto| {
            let path = dir.join(proto);
            path
        })
        .collect();

    tonic_build::configure()
        .build_client(true)
        .build_server(true)
        .out_dir("./src")
        .type_attribute("InfoResponse", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("WalletBalanceResponse", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("NewAddressResponse", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("Peer", "#[derive(serde::Serialize, serde::Deserialize)]")
        .compile(&proto_paths, &[dir])?;

    Ok(())
}
