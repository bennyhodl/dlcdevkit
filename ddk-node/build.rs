fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/proto/ddkrpc.proto");
    let dir = std::path::PathBuf::from("./src/proto");

    let protos = ["ddkrpc.proto"];

    let proto_paths: Vec<_> = protos.iter().map(|proto| dir.join(proto)).collect();

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .compile_protos(&proto_paths, &[dir])?;

    Ok(())
}
