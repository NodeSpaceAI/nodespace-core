//! The environment of the programs the daemon and its tray start.
//!
//! The daemon's own environment can hold credentials, so no program it starts
//! inherits it whole. Each is given the variables on an allow-list that are set
//! in the daemon, and nothing else. The programs the agent starts (the agent's
//! terminal child and the tool programs) are cleared in the `nodespace-agent`
//! crate, which keeps their own lists; every other spawn in this crate goes
//! through [`scrubbed`].

use std::ffi::OsString;
use std::process::Command;

/// What every child needs to find programs, the user's home and a temporary
/// directory, and what the Windows loader and shell need to start anything.
const BASE: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "TMPDIR",
    "SystemRoot",
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
];

/// What the desktop app needs on top of [`BASE`] to open a window and find its
/// own data: the display session, the Windows profile folders, and the
/// product's own settings (`NODESPACE_*`, `NODESPACED_*`, which name locations
/// and switches and carry no credential), plus the locale and XDG families.
const UI_NAMES: &[&str] = &[
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "DBUS_SESSION_BUS_ADDRESS",
    "SHELL",
    "TERM",
    "USERPROFILE",
    "USERNAME",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "TEMP",
    "TMP",
    "HOMEDRIVE",
    "HOMEPATH",
    "__CF_USER_TEXT_ENCODING",
];

const UI_PREFIXES: &[&str] = &["NODESPACE_", "NODESPACED_", "XDG_", "LC_"];

/// Looks a variable up in the daemon's environment.
type Lookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

/// A command for `program` whose environment is cleared and rebuilt from
/// [`BASE`] alone: for helper programs (`ps`, `taskkill`) that need nothing else.
pub(crate) fn scrubbed(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    apply(&mut command, &|name| std::env::var_os(name), false);
    command
}

/// A command for the desktop app: [`BASE`] and the app's own names.
pub(crate) fn scrubbed_ui(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    apply(&mut command, &|name| std::env::var_os(name), true);
    command
}

fn apply(command: &mut Command, lookup: Lookup<'_>, ui: bool) {
    command.env_clear();
    for name in BASE {
        if let Some(value) = lookup(name) {
            command.env(name, value);
        }
    }
    if ui {
        for name in UI_NAMES {
            if let Some(value) = lookup(name) {
                command.env(name, value);
            }
        }
        for (name, value) in std::env::vars_os() {
            let Some(name) = name.to_str() else { continue };
            if UI_PREFIXES.iter().any(|p| name.starts_with(p)) && !BASE.contains(&name) {
                command.env(name, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Source files of this crate, with the text before their first test module.
    fn production_sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let production = text
                        .split("#[cfg(test)]")
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    out.push((path.display().to_string(), production));
                }
            }
        }
        let mut out = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut out,
        );
        out
    }

    /// Every program this crate starts goes through this module: a bare
    /// `Command::new` anywhere else would hand the daemon's environment on.
    #[test]
    fn no_other_production_code_starts_a_program_with_the_daemons_environment() {
        let needle = ["Command", "::new("].concat();
        let sources = production_sources();
        assert!(sources.len() > 5, "the scan reaches the crate's files");
        for (file, text) in sources {
            if file.ends_with("child_env.rs") {
                continue;
            }
            assert!(
                !text.contains(&needle),
                "{file} starts a program with `{needle}`; use child_env::scrubbed"
            );
        }
    }

    #[cfg(unix)]
    fn printed_env(mut command: Command) -> String {
        command.stdout(std::process::Stdio::piped());
        String::from_utf8(command.output().expect("env runs").stdout).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn a_helper_program_gets_the_base_variables_and_nothing_else() {
        // The test process always has variables outside the list (cargo's own).
        assert!(std::env::vars_os().any(|(n, _)| n.to_str().is_some_and(|n| !BASE.contains(&n))));
        let out = printed_env(scrubbed("/usr/bin/env"));
        for line in out.lines() {
            let name = line.split('=').next().unwrap();
            assert!(BASE.contains(&name), "{name} reached the helper program");
        }
        assert!(out.lines().any(|l| l.starts_with("PATH=")));
    }

    #[cfg(unix)]
    #[test]
    fn the_app_gets_its_own_names_and_not_credentials() {
        // A fixed lookup stands in for the daemon's environment.
        let planted: std::collections::HashMap<&str, &str> = [
            ("PATH", "/bin"),
            ("DISPLAY", ":0"),
            ("OPENAI_API_KEY", "sk-secret"),
        ]
        .into_iter()
        .collect();
        let mut command = Command::new("/usr/bin/env");
        apply(&mut command, &|n| planted.get(n).map(OsString::from), true);
        let out = printed_env(command);
        assert!(out.lines().any(|l| l == "DISPLAY=:0"));
        assert!(out.lines().any(|l| l == "PATH=/bin"));
        assert!(!out.contains("OPENAI_API_KEY"));
        // Names with the product's prefix come from the real environment.
        let mut real = scrubbed_ui("/usr/bin/env");
        real.env("NODESPACE_X", "1");
        let out = printed_env(real);
        for line in out.lines() {
            let name = line.split('=').next().unwrap();
            assert!(
                BASE.contains(&name)
                    || UI_NAMES.contains(&name)
                    || UI_PREFIXES.iter().any(|p| name.starts_with(p)),
                "{name} reached the app"
            );
        }
    }
}
