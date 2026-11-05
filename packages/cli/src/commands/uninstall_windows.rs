//! What `nodespace uninstall` does on Windows beyond deleting files: stop the
//! daemon and remove its `HKCU` autorun entry. It uses only `powershell.exe`
//! and `reg.exe`, which every Windows install has, so it needs no extra crate.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;

/// Stops every running process whose image is named in `image_names` and whose
/// executable sits under one of `bin_dirs`, then waits for each to exit.
/// Returns how many it stopped.
///
/// A process is matched by its full executable path, not only its name, so a
/// daemon launched from somewhere else (a source checkout, another install) is
/// left alone. The match is on the executable's directory prefix and ignores
/// case, as Windows paths do. Names and directories travel in environment
/// variables so no quoting can change what the script matches.
pub fn stop_daemons(image_names: &[&str], bin_dirs: &[PathBuf]) -> Result<usize> {
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$names = @($env:NS_UNINSTALL_IMAGES -split ';' | Where-Object { $_ })
$dirs = @($env:NS_UNINSTALL_DIRS -split ';' | Where-Object { $_ } | ForEach-Object { $_.TrimEnd('\') + '\' })
$found = @()
foreach ($p in Get-CimInstance Win32_Process) {
  if (-not $p.ExecutablePath) { continue }
  if (-not ($names -contains $p.Name)) { continue }
  foreach ($d in $dirs) {
    if ($p.ExecutablePath.StartsWith($d, [System.StringComparison]::OrdinalIgnoreCase)) { $found += $p; break }
  }
}
foreach ($p in $found) { Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue }
foreach ($p in $found) {
  Wait-Process -Id $p.ProcessId -Timeout 10 -ErrorAction SilentlyContinue
  if (Get-Process -Id $p.ProcessId -ErrorAction SilentlyContinue) { [Console]::Error.WriteLine("process " + $p.ProcessId + " (" + $p.ExecutablePath + ") did not exit"); exit 1 }
}
[Console]::Out.WriteLine($found.Count)
"#;
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("NS_UNINSTALL_IMAGES", image_names.join(";"))
        .env(
            "NS_UNINSTALL_DIRS",
            bin_dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(";"),
        )
        .output()
        .context("Failed to run powershell.exe to stop the daemon")?;
    if !output.status.success() {
        bail!(
            "Could not stop the daemon: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .context("powershell.exe did not report how many daemons it stopped")
}

/// Removes the value `value` under `HKCU\<key>`. Returns `Ok(true)` when it was
/// there and is now gone, `Ok(false)` when it was never there.
///
/// Presence is checked first because `reg delete` exits non-zero for a missing
/// value exactly as it does for a real failure.
pub fn remove_autorun(key: &str, value: &str) -> Result<bool> {
    let key = format!("HKCU\\{key}");
    let present = Command::new("reg")
        .args(["query", &key, "/v", value])
        .output()
        .context("Failed to run reg.exe to look for the autorun entry")?
        .status
        .success();
    if !present {
        return Ok(false);
    }
    let out = Command::new("reg")
        .args(["delete", &key, "/v", value, "/f"])
        .output()
        .context("Failed to run reg.exe to remove the autorun entry")?;
    if out.status.success() {
        Ok(true)
    } else {
        bail!(
            "Could not remove the autorun entry {key}\\{value}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
}
