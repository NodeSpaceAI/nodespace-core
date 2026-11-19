fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", protoc);
    // The one message that carries a credential value must not print it: its
    // `Debug` is written by hand in `lib.rs`.
    tonic_build::configure()
        .skip_debug("nodespace.SetSecretRequest")
        .compile_protos(
            &[
                "proto/node_service.proto",
                "proto/agent_session_service.proto",
                "proto/import_service.proto",
                "proto/local_agent_service.proto",
                "proto/database_service.proto",
                "proto/secret_service.proto",
            ],
            &["proto"],
        )?;
    Ok(())
}
