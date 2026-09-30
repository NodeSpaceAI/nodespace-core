use std::env;

/// Sets a `cfg` alias for this crate, declaring it to `rustc`'s cfg checker
/// too — the same two lines `tauri_build` prints for its own `desktop` and
/// `mobile` aliases.
fn cfg_alias(alias: &str, enabled: bool) {
    println!("cargo:rustc-check-cfg=cfg({alias})");
    if enabled {
        println!("cargo:rustc-cfg={alias}");
    }
}

fn main() {
    // Compile the Pro-tier proto package. The .proto lives under `proto/` in
    // this crate (vendored from `nodespace-sync/nodespaced-pro/proto/`). When
    // sync is checked out as a sibling, `scripts/refresh-pro-proto.ts`
    // re-vendors from the source-of-truth.
    let protoc = protoc_bin_vendored::protoc_bin_path()
        .expect("protoc-bin-vendored is required for the Pro proto build");
    // `set_var` is fine on edition 2021 — build scripts are
    // single-threaded by Cargo's contract. Once the workspace bumps
    // to edition 2024 (or tonic-build past 0.12 lands a
    // `protoc_executable` builder), switch to the builder-method
    // form to stay forward-compatible without the `unsafe` wrap
    // edition 2024 will require for env mutation.
    std::env::set_var("PROTOC", &protoc);
    tonic_build::configure()
        .build_server(false) // Tauri client only; daemon defines the server.
        .compile_protos(&["proto/nodespace_pro.proto"], &["proto"])
        .expect("failed to compile nodespace.pro.v1 proto");

    println!("cargo:rerun-if-changed=proto/nodespace_pro.proto");

    // `#[cfg(desktop)]` guards the single-instance plugin in `run()`. Only
    // `tauri_build` defines the `desktop` and `mobile` aliases, and this
    // library does not call it (the app crate that owns tauri.conf.json does),
    // so without them that code compiles out silently and the single-instance
    // guard disappears. This mirrors `tauri_build`'s own definition: desktop
    // unless the target OS is iOS or Android.
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("cargo always sets CARGO_CFG_TARGET_OS");
    let mobile = target_os == "ios" || target_os == "android";
    cfg_alias("desktop", !mobile);
    cfg_alias("mobile", mobile);
}
