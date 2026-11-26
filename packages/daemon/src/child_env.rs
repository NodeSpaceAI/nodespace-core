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
    // Linux: the session user's other name, the locale list GTK reads, and the
    // certificate locations a TLS stack consults (paths, not credentials).
    "LOGNAME",
    "LANGUAGE",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NO_PROXY",
    "no_proxy",
    // Windows: where the program folders are, the machine's architecture and
    // core count (WebView2 and the Rust runtime size themselves from them), the
    // session and the system drive.
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "CommonProgramW6432",
    "ALLUSERSPROFILE",
    "PUBLIC",
    "SystemDrive",
    "COMPUTERNAME",
    "USERDOMAIN",
    "SESSIONNAME",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_ARCHITEW6432",
    "NUMBER_OF_PROCESSORS",
    "OS",
];

/// Variable families the app reads: the product's own, XDG and locale, and the
/// GTK, GDK and WebKit switches the Linux webview honours (`GDK_BACKEND`,
/// `GTK_THEME`, `WEBKIT_DISABLE_DMABUF_RENDERER` and the like).
///
/// Left out on purpose: `LD_LIBRARY_PATH` (it redirects which libraries the app
/// loads), and `HTTP_PROXY`, `HTTPS_PROXY` and `ALL_PROXY` (their URLs can embed
/// a user name and password). The credential-bearing ones stay with the daemon.
const UI_PREFIXES: &[&str] = &[
    "NODESPACE_",
    "NODESPACED_",
    "XDG_",
    "LC_",
    "GTK_",
    "GDK_",
    "WEBKIT_",
];

/// Looks a variable up in the daemon's environment.
type Lookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

/// Every variable of the daemon's environment, for the prefix families.
type AllVars<'a> = &'a dyn Fn() -> Vec<(OsString, OsString)>;

/// A command for `program` whose environment is cleared and rebuilt from
/// [`BASE`] alone: for helper programs (`ps`, `taskkill`) that need nothing else.
pub(crate) fn scrubbed(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    apply(
        &mut command,
        &|name| std::env::var_os(name),
        &|| std::env::vars_os().collect(),
        false,
    );
    command
}

/// A command for the desktop app: [`BASE`] and the app's own names.
pub(crate) fn scrubbed_ui(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    apply(
        &mut command,
        &|name| std::env::var_os(name),
        &|| std::env::vars_os().collect(),
        true,
    );
    command
}

fn apply(command: &mut Command, lookup: Lookup<'_>, all: AllVars<'_>, ui: bool) {
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
        for (name, value) in all() {
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

    use proc_macro2::{TokenStream, TokenTree};
    use syn::visit::Visit;

    /// Finds every place a source file starts a program without
    /// [`scrubbed`]: a path `Command::new` (or `std::process::Command::new`,
    /// or through a name `Command` was imported or aliased as), whether
    /// called, passed on as a value, or written inside a macro's arguments.
    /// Items marked `#[cfg(test)]` are skipped, wherever they sit in the file
    /// (the scan reads the syntax tree, so an early test-only item does not
    /// hide what follows it, and spacing does not matter).
    struct SpawnScan {
        /// Names that stand for `Command`: `Command` and its renames.
        names: Vec<String>,
        hits: Vec<String>,
    }

    fn is_test_only(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| match &a.meta {
            syn::Meta::List(list) if list.path.is_ident("cfg") => {
                let text = list.tokens.to_string();
                text.split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|word| word == "test")
                    && !text.contains("not")
            }
            _ => false,
        })
    }

    impl SpawnScan {
        fn note_use(&mut self, tree: &syn::UseTree) {
            match tree {
                syn::UseTree::Path(p) => self.note_use(&p.tree),
                syn::UseTree::Group(g) => g.items.iter().for_each(|t| self.note_use(t)),
                syn::UseTree::Name(n) if n.ident == "Command" => {}
                syn::UseTree::Rename(r) if r.ident == "Command" => {
                    self.names.push(r.rename.to_string());
                }
                _ => {}
            }
        }

        fn is_command_path(&self, path: &syn::Path) -> bool {
            let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
            segments.len() >= 2
                && segments[segments.len() - 1] == "new"
                && self.names.contains(&segments[segments.len() - 2])
        }

        fn scan_tokens(&mut self, tokens: TokenStream) {
            let trees: Vec<TokenTree> = tokens.into_iter().collect();
            for (i, tree) in trees.iter().enumerate() {
                match tree {
                    TokenTree::Group(g) => self.scan_tokens(g.stream()),
                    TokenTree::Ident(id) if self.names.contains(&id.to_string()) => {
                        let rest: Vec<String> = trees[i + 1..]
                            .iter()
                            .take(3)
                            .map(|t| t.to_string())
                            .collect();
                        if rest == [":", ":", "new"] {
                            self.hits.push(format!("{id}::new in a macro"));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    impl<'ast> Visit<'ast> for SpawnScan {
        fn visit_item_use(&mut self, i: &'ast syn::ItemUse) {
            if !is_test_only(&i.attrs) {
                self.note_use(&i.tree);
            }
        }
        fn visit_item_type(&mut self, i: &'ast syn::ItemType) {
            if let syn::Type::Path(p) = &*i.ty {
                if p.path.segments.last().is_some_and(|s| s.ident == "Command") {
                    self.names.push(i.ident.to_string());
                }
            }
        }
        fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
            if !is_test_only(&i.attrs) {
                syn::visit::visit_item_mod(self, i);
            }
        }
        fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
            if !is_test_only(&i.attrs) {
                syn::visit::visit_item_fn(self, i);
            }
        }
        fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
            if !is_test_only(&i.attrs) {
                syn::visit::visit_item_impl(self, i);
            }
        }
        fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
            if !is_test_only(&i.attrs) {
                syn::visit::visit_impl_item_fn(self, i);
            }
        }
        fn visit_local(&mut self, i: &'ast syn::Local) {
            if !is_test_only(&i.attrs) {
                syn::visit::visit_local(self, i);
            }
        }
        fn visit_expr_path(&mut self, i: &'ast syn::ExprPath) {
            if self.is_command_path(&i.path) {
                self.hits.push(path_text(&i.path));
            }
            syn::visit::visit_expr_path(self, i);
        }
        fn visit_macro(&mut self, i: &'ast syn::Macro) {
            self.scan_tokens(i.tokens.clone());
        }
    }

    fn path_text(path: &syn::Path) -> String {
        path.segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::")
    }

    /// The program-starting paths in `source` outside `#[cfg(test)]` items.
    fn spawns_in(source: &str) -> Vec<String> {
        let file = syn::parse_file(source).expect("the source parses");
        let mut scan = SpawnScan {
            names: vec!["Command".to_string()],
            hits: Vec::new(),
        };
        // Imports come before their uses in a file, but an alias can sit below
        // the item that uses it: collect them in a first pass.
        scan.visit_file(&file);
        scan.hits.clear();
        scan.visit_file(&file);
        scan.hits
    }

    fn crate_sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push((
                        path.display().to_string(),
                        std::fs::read_to_string(&path).unwrap(),
                    ));
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
        let sources = crate_sources();
        assert!(sources.len() > 5, "the scan reaches the crate's files");
        for (file, text) in sources {
            if file.ends_with("child_env.rs") {
                continue;
            }
            let hits = spawns_in(&text);
            assert!(
                hits.is_empty(),
                "{file} starts a program with {hits:?}; use child_env::scrubbed"
            );
        }
    }

    #[test]
    fn the_scan_finds_a_spawn_after_an_early_test_only_item() {
        let source = r#"
            #[cfg(test)]
            use something::Helper;
            fn run() { std::process::Command::new("sh"); }
        "#;
        assert_eq!(spawns_in(source), ["std::process::Command::new"]);
    }

    #[test]
    fn the_scan_finds_a_spawn_through_an_alias_or_with_spacing() {
        for source in [
            "use std::process::Command as C; fn f() { C::new(\"x\"); }",
            "use std::process::Command; fn f() { Command::new (\"x\"); }",
            "use tokio::process::{Command as Cmd, Stdio}; fn f() { let _ = Cmd :: new; }",
            "use std::process::Command; type C = Command; fn f() { C::new(\"x\"); }",
            "use std::process::Command; fn f() { vec![Command::new(\"x\")]; }",
            "fn f() { std::process::Command::new(\n\"x\"); }",
        ] {
            assert!(!spawns_in(source).is_empty(), "missed: {source}");
        }
    }

    #[test]
    fn the_scan_ignores_test_only_items_and_other_types() {
        let source = r#"
            use std::process::Command;
            #[cfg(test)]
            mod tests { fn t() { Command::new("true"); } }
            #[cfg(test)]
            fn helper() { Command::new("true"); }
            struct Other;
            impl Other { fn new() -> Self { Other } }
            fn f() { let _ = Other::new(); let _ = scrubbed("ps"); }
        "#;
        assert_eq!(spawns_in(source), Vec::<String>::new());
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
        apply(
            &mut command,
            &|n| planted.get(n).map(OsString::from),
            &|| {
                planted
                    .iter()
                    .map(|(n, v)| (OsString::from(n), OsString::from(v)))
                    .collect()
            },
            true,
        );
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

    /// The environment a program started through [`scrubbed_ui`] receives from
    /// a daemon whose environment is `vars`.
    #[cfg(unix)]
    fn ui_env_from(vars: &[(&str, &str)]) -> Vec<String> {
        let planted: std::collections::HashMap<&str, &str> = vars.iter().copied().collect();
        let mut command = Command::new("/usr/bin/env");
        apply(
            &mut command,
            &|n| planted.get(n).map(OsString::from),
            &|| {
                vars.iter()
                    .map(|(n, v)| (OsString::from(n), OsString::from(v)))
                    .collect()
            },
            true,
        );
        printed_env(command).lines().map(str::to_string).collect()
    }

    /// What the app needs to open a window on Linux (GTK, GDK and WebKit
    /// switches, the locale list, the login name) and on Windows (program
    /// folders, architecture, core count, session): all reach it, and the
    /// credentials the daemon may hold do not. The platform lists are read
    /// from the code; this proves their effect on any host.
    #[cfg(unix)]
    #[test]
    fn the_app_gets_the_linux_and_windows_variables_it_needs() {
        let needed = [
            ("GDK_BACKEND", "x11"),
            ("GDK_SCALE", "2"),
            ("GTK_THEME", "Adwaita:dark"),
            ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
            ("LANGUAGE", "en"),
            ("LOGNAME", "me"),
            ("SSL_CERT_FILE", "/etc/ssl/cert.pem"),
            ("NO_PROXY", "localhost"),
            ("ProgramFiles", "C:\\Program Files"),
            ("ProgramFiles(x86)", "C:\\Program Files (x86)"),
            ("ProgramW6432", "C:\\Program Files"),
            ("PROCESSOR_ARCHITECTURE", "AMD64"),
            ("NUMBER_OF_PROCESSORS", "8"),
            ("OS", "Windows_NT"),
            ("SystemDrive", "C:"),
        ];
        let withheld = [
            ("HTTPS_PROXY", "http://user:pw@proxy:8080"),
            ("LD_LIBRARY_PATH", "/opt/lib"),
            ("GITHUB_TOKEN", "ghp_x"),
        ];
        let all: Vec<(&str, &str)> = needed.iter().chain(withheld.iter()).copied().collect();
        let got = ui_env_from(&all);
        for (name, value) in needed {
            assert!(
                got.contains(&format!("{name}={value}")),
                "{name} did not reach the app"
            );
        }
        for (name, _) in withheld {
            assert!(
                !got.iter().any(|l| l.starts_with(&format!("{name}="))),
                "{name} reached the app"
            );
        }
    }
}
