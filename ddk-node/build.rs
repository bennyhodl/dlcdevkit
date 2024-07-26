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
        .compile(&proto_paths, &[dir])?;

    Ok(())
}
