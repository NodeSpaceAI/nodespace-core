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
