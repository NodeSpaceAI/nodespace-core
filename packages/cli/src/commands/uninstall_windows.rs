//! What `nodespace uninstall` does on Windows beyond deleting files: stop the
//! daemon, remove its `HKCU` autorun entry, and remove the files this very
//! process is running from once it has exited. It uses only `powershell.exe`
//! and `reg.exe`, which every Windows install has, so it needs no extra crate.
//! Both are started from the system directory, never found through `PATH`.

use anyhow::{bail, Context, Result};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// `CREATE_NO_WINDOW`: no console window for a child console program.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// `CREATE_NEW_PROCESS_GROUP`: the child is not stopped with this console.
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// A program under `%SystemRoot%\System32`.
fn system_tool(relative: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("windir"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("System32").join(relative)
}

fn reg_exe() -> PathBuf {
    system_tool("reg.exe")
}

fn powershell_exe() -> PathBuf {
    system_tool(r"WindowsPowerShell\v1.0\powershell.exe")
}

/// Joins `items` for an environment variable. A newline cannot occur in a
/// Windows path or image name, so the script can split on it without any
/// quoting rule that could change what is matched.
fn newline_list<T: AsRef<str>>(items: &[T]) -> String {
    items
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join("\n")
}

fn path_list(paths: &[PathBuf]) -> String {
    newline_list(
        &paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>(),
    )
}

/// Stops every running process whose image is named in `image_names` and whose
/// executable sits under one of `bin_dirs`, then waits for each to exit.
/// Returns how many it stopped.
///
/// A process is matched by its full executable path, not only its name, so a
/// daemon launched from somewhere else (a source checkout, another install) is
/// left alone. The match is on the executable's directory prefix and ignores
/// case, as Windows paths do.
pub fn stop_daemons(image_names: &[&str], bin_dirs: &[PathBuf]) -> Result<usize> {
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$names = @($env:NS_UNINSTALL_IMAGES -split "`n" | Where-Object { $_ })
$dirs = @($env:NS_UNINSTALL_DIRS -split "`n" | Where-Object { $_ } | ForEach-Object { $_.TrimEnd('\') + '\' })
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
    let output = Command::new(powershell_exe())
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("NS_UNINSTALL_IMAGES", newline_list(image_names))
        .env("NS_UNINSTALL_DIRS", path_list(bin_dirs))
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

/// Starts a hidden helper that waits for process `parent_pid` to exit and then
/// removes `files` (retrying while Windows still holds them) and, after them,
/// each of `dirs` that is empty. For the files this process is running from,
/// which it cannot delete itself.
///
/// The helper is not tied to this process: it has no window and its own
/// process group, so it outlives the exit it waits for.
pub fn remove_after_exit(parent_pid: u32, targets: &[PathBuf]) -> Result<()> {
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$all = @($env:NS_UNINSTALL_TARGETS -split "`n" | Where-Object { $_ })
Wait-Process -Id ([int]$env:NS_UNINSTALL_PARENT) -Timeout 120
for ($i = 0; $i -lt 50; $i++) {
  $left = $false
  foreach ($t in $all) {
    if (Test-Path -LiteralPath $t -PathType Leaf) {
      try { [System.IO.File]::Delete($t) } catch { }
      if (Test-Path -LiteralPath $t -PathType Leaf) { $left = $true }
    }
  }
  if (-not $left) { break }
  Start-Sleep -Milliseconds 200
}
foreach ($t in $all) {
  if (Test-Path -LiteralPath $t -PathType Container) { try { [System.IO.Directory]::Delete($t) } catch { } }
}
"#;
    Command::new(powershell_exe())
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            SCRIPT,
        ])
        .env("NS_UNINSTALL_PARENT", parent_pid.to_string())
        .env("NS_UNINSTALL_TARGETS", path_list(targets))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .context("Failed to start the helper that removes the running executable")?;
    Ok(())
}

/// Removes the value `value` under `HKCU\<key>`. Returns `Ok(true)` when it was
/// there and is now gone, `Ok(false)` when it was never there.
///
/// `reg query` exits 1 both for a missing value and for a real failure, and its
/// message is localised, so the two are told apart without reading it: the key
/// itself must query successfully, and then a value query that exits 1 means
/// the value is absent. Any other outcome is an error to report.
pub fn remove_autorun(key: &str, value: &str) -> Result<bool> {
    let key = format!("HKCU\\{key}");
    let query = |args: &[&str]| {
        Command::new(reg_exe())
            .args(args)
            .output()
            .context("Failed to run reg.exe to look for the autorun entry")
    };
    let key_out = query(&["query", &key])?;
    if !key_out.status.success() {
        bail!(
            "Could not read the registry key {key}: {}",
            String::from_utf8_lossy(&key_out.stderr).trim()
        );
    }
    let value_out = query(&["query", &key, "/v", value])?;
    match value_out.status.code() {
        Some(0) => {}
        Some(1) => return Ok(false),
        _ => bail!(
            "Could not read the autorun entry {key}\\{value}: {}",
            String::from_utf8_lossy(&value_out.stderr).trim()
        ),
    }
    let out = Command::new(reg_exe())
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
