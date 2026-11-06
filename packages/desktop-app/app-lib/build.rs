use std::env;
use std::path::Path;
use std::process::Command;

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
    if target_os == "windows" {
        embed_common_controls_manifest();
    }
}

/// Application manifest asking for Common Controls 6. The GUI dependencies
/// import `TaskDialogIndirect`, which only the version 6 `comctl32` exports;
/// without the manifest the Windows loader binds the legacy library and the
/// test executables abort at start (STATUS_ENTRYPOINT_NOT_FOUND). The app
/// binary gets this from the Tauri build step; the test executables of this
/// crate do not go through that step, so they get it here.
const COMMON_CONTROLS_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;

/// Links the manifest into this package's executables (its tests). The crate is
/// an rlib, so the link arguments never reach the app binary.
fn embed_common_controls_manifest() {
    let out_dir = env::var("OUT_DIR").expect("cargo always sets OUT_DIR");
    let manifest = Path::new(&out_dir).join("common-controls.manifest");
    std::fs::write(&manifest, COMMON_CONTROLS_MANIFEST).expect("write manifest into OUT_DIR");
    let abi = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if abi == "msvc" {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        return;
    }
    // GNU toolchain: the linker has no manifest option, so compile a resource
    // object with `windres` and link that.
    let rc = Path::new(&out_dir).join("common-controls.rc");
    let object = Path::new(&out_dir).join("common-controls.o");
    let manifest_for_rc = manifest.display().to_string().replace('\\', "/");
    std::fs::write(&rc, format!("1 24 \"{manifest_for_rc}\"\n")).expect("write rc into OUT_DIR");
    let windres = env::var("WINDRES").unwrap_or_else(|_| "windres".to_string());
    let status = Command::new(&windres)
        .arg("--input")
        .arg(&rc)
        .arg("--output")
        .arg(&object)
        .arg("--output-format=coff")
        .status();
    match status {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg={}", object.display()),
        _ => println!(
            "cargo:warning=`{windres}` failed or was not found; test executables will not start \
             on Windows without a Common Controls 6 manifest (set WINDRES)"
        ),
    }
}
