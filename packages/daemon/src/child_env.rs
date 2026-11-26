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
    // The Linux webview's display switches, each named: the GTK, GDK and
    // WebKit families also hold variables that load code (`GTK_MODULES`,
    // `GTK_PATH`, `GTK_EXE_PREFIX`, `GTK_IM_MODULE_FILE`,
    // `GDK_PIXBUF_MODULE_FILE`, `GDK_PIXBUF_MODULEDIR`, `WEBKIT_EXEC_PATH`,
    // `WEBKIT_INJECTED_BUNDLE_PATH`) or open a debugging port
    // (`WEBKIT_INSPECTOR_SERVER`), so no prefix passes them.
    "GDK_BACKEND",
    "GDK_SCALE",
    "GDK_DPI_SCALE",
    "GTK_THEME",
    "GTK_USE_PORTAL",
    "WEBKIT_DISABLE_COMPOSITING_MODE",
    "WEBKIT_DISABLE_DMABUF_RENDERER",
];

/// Variable families the app reads: the product's own, XDG and locale.
///
/// Left out on purpose: `LD_LIBRARY_PATH` and the GTK, GDK and WebKit
/// code-loading variables (see [`UI_NAMES`]), which redirect what the app
/// loads, and `HTTP_PROXY`, `HTTPS_PROXY` and `ALL_PROXY` (their URLs can embed
/// a user name and password). These stay with the daemon.
const UI_PREFIXES: &[&str] = &["NODESPACE_", "NODESPACED_", "XDG_", "LC_"];

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

    // Scope: this is a scan for `Command::new` (std or tokio) in the
    // production source of packages/daemon/src only. It does not see
    // posix_spawn, system, execve, openers (open::that, webbrowser), build.rs
    // or tests/it. A different type that is also named `Command` (clap's) is
    // flagged too: that over-flags loudly rather than missing a spawn.

    /// Whether `attrs` mark an item as compiled only for tests: exactly
    /// `#[cfg(test)]` or `#[cfg(all(test, ..))]`. `cfg(any(test, unix))`,
    /// `cfg(not(test))` and the like also build outside tests, so they are
    /// scanned.
    fn is_test_only(attrs: &[syn::Attribute]) -> bool {
        use syn::punctuated::Punctuated;
        attrs.iter().any(|a| {
            let syn::Meta::List(list) = &a.meta else {
                return false;
            };
            if !list.path.is_ident("cfg") {
                return false;
            }
            let Ok(args) =
                list.parse_args_with(Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated)
            else {
                return false;
            };
            let args: Vec<&syn::Meta> = args.iter().collect();
            match args.as_slice() {
                [syn::Meta::Path(p)] => p.is_ident("test"),
                [syn::Meta::List(all)] if all.path.is_ident("all") => all
                    .parse_args_with(Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated)
                    .is_ok_and(|inner| {
                        inner
                            .iter()
                            .any(|m| matches!(m, syn::Meta::Path(p) if p.is_ident("test")))
                    }),
                _ => false,
            }
        })
    }

    /// Collects `use .. Command as X`, `type X = Command` and renames of those,
    /// across every file given, to a fixed point.
    fn command_names(files: &[syn::File]) -> Vec<String> {
        #[derive(Default)]
        struct Pairs(Vec<(String, String)>);
        impl Pairs {
            fn tree(&mut self, tree: &syn::UseTree) {
                match tree {
                    syn::UseTree::Path(p) => self.tree(&p.tree),
                    syn::UseTree::Group(g) => g.items.iter().for_each(|t| self.tree(t)),
                    syn::UseTree::Rename(r) => {
                        self.0.push((r.ident.to_string(), r.rename.to_string()));
                    }
                    _ => {}
                }
            }
        }
        impl<'ast> Visit<'ast> for Pairs {
            fn visit_item_use(&mut self, i: &'ast syn::ItemUse) {
                self.tree(&i.tree);
            }
            fn visit_item_type(&mut self, i: &'ast syn::ItemType) {
                if let syn::Type::Path(p) = &*i.ty {
                    if let Some(last) = p.path.segments.last() {
                        self.0.push((last.ident.to_string(), i.ident.to_string()));
                    }
                }
            }
        }
        let mut pairs = Pairs::default();
        for file in files {
            pairs.visit_file(file);
        }
        let mut names = vec!["Command".to_string()];
        loop {
            let before = names.len();
            for (target, alias) in &pairs.0 {
                if names.contains(target) && !names.contains(alias) {
                    names.push(alias.clone());
                }
            }
            if names.len() == before {
                return names;
            }
        }
    }

    /// Finds the places a file starts a program without [`scrubbed`]: a path
    /// `Name::new` (or `std::process::Command::new`, `<Name>::new`) for any
    /// `Name` that stands for `Command`, whether called, passed on as a value,
    /// or written inside a macro's arguments. Items marked `#[cfg(test)]` are
    /// skipped wherever they sit (the scan reads the syntax tree, so an early
    /// test-only item does not hide what follows it, and spacing does not
    /// matter).
    struct SpawnScan<'n> {
        names: &'n [String],
        hits: Vec<String>,
    }

    impl SpawnScan<'_> {
        fn is_command_path(&self, path: &syn::Path) -> bool {
            let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
            segments.len() >= 2
                && segments[segments.len() - 1] == "new"
                && self.names.contains(&segments[segments.len() - 2])
        }

        fn is_command_type(&self, ty: &syn::Type) -> bool {
            match ty {
                syn::Type::Path(p) => p
                    .path
                    .segments
                    .last()
                    .is_some_and(|s| self.names.contains(&s.ident.to_string())),
                syn::Type::Paren(p) => self.is_command_type(&p.elem),
                syn::Type::Group(g) => self.is_command_type(&g.elem),
                _ => false,
            }
        }

        fn scan_tokens(&mut self, tokens: TokenStream) {
            let trees: Vec<TokenTree> = tokens.into_iter().collect();
            for (i, tree) in trees.iter().enumerate() {
                match tree {
                    TokenTree::Group(g) => self.scan_tokens(g.stream()),
                    TokenTree::Ident(id) => {
                        let rest: Vec<String> = trees[i + 1..]
                            .iter()
                            .take(3)
                            .map(|t| t.to_string())
                            .collect();
                        if rest != [":", ":", "new"] {
                            continue;
                        }
                        // A name for Command, or a macro fragment (`$c::new`)
                        // that may stand for it.
                        let fragment = i > 0 && trees[i - 1].to_string() == "$";
                        if fragment || self.names.contains(&id.to_string()) {
                            self.hits.push(format!("{id}::new in a macro"));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    impl<'ast> Visit<'ast> for SpawnScan<'_> {
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
            let qualified = i.qself.as_ref().is_some_and(|q| {
                self.is_command_type(&q.ty)
                    && i.path.segments.last().is_some_and(|s| s.ident == "new")
            });
            if self.is_command_path(&i.path) || qualified {
                self.hits.push(
                    i.path
                        .segments
                        .iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::"),
                );
            }
            syn::visit::visit_expr_path(self, i);
        }
        fn visit_macro(&mut self, i: &'ast syn::Macro) {
            self.scan_tokens(i.tokens.clone());
        }
    }

    /// The program-starting paths in the `sources` (name, text) pairs outside
    /// `#[cfg(test)]` items, as (file, path).
    fn spawns_in(sources: &[(String, String)]) -> Vec<(String, String)> {
        let files: Vec<syn::File> = sources
            .iter()
            .map(|(name, text)| {
                syn::parse_file(text).unwrap_or_else(|e| panic!("{name} does not parse: {e}"))
            })
            .collect();
        let names = command_names(&files);
        let mut out = Vec::new();
        for ((name, _), file) in sources.iter().zip(&files) {
            let mut scan = SpawnScan {
                names: &names,
                hits: Vec::new(),
            };
            scan.visit_file(file);
            out.extend(scan.hits.into_iter().map(|h| (name.clone(), h)));
        }
        out
    }

    fn one(source: &str) -> Vec<String> {
        spawns_in(&[("t.rs".to_string(), source.to_string())])
            .into_iter()
            .map(|(_, h)| h)
            .collect()
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

    /// Every program this crate starts goes through this module: a
    /// `Command::new` anywhere else would hand the daemon's environment on.
    /// Scope: see the note above (packages/daemon/src, `Command::new` only).
    #[test]
    fn no_other_production_code_starts_a_program_with_the_daemons_environment() {
        let sources: Vec<_> = crate_sources()
            .into_iter()
            .filter(|(f, _)| !f.ends_with("child_env.rs"))
            .collect();
        assert!(sources.len() > 5, "the scan reaches the crate's files");
        let hits = spawns_in(&sources);
        assert!(
            hits.is_empty(),
            "these start a program with a bare Command::new; use child_env::scrubbed: {hits:?}"
        );
    }

    #[test]
    fn the_scan_finds_a_spawn_after_an_early_test_only_item() {
        let source = r#"
            #[cfg(test)]
            use something::Helper;
            fn run() { std::process::Command::new("sh"); }
        "#;
        assert_eq!(one(source), ["std::process::Command::new"]);
    }

    #[test]
    fn the_scan_finds_a_spawn_through_an_alias_spacing_or_a_macro() {
        for source in [
            "use std::process::Command as C; fn f() { C::new(\"x\"); }",
            "use std::process::Command; fn f() { Command::new (\"x\"); }",
            "use tokio::process::{Command as Cmd, Stdio}; fn f() { let _ = Cmd :: new; }",
            "use std::process::Command; type C = Command; fn f() { C::new(\"x\"); }",
            "use std::process::Command; fn f() { vec![Command::new(\"x\")]; }",
            "fn f() { std::process::Command::new(\n\"x\"); }",
            "use std::process::Command; fn f() { <Command>::new(\"x\"); }",
            "fn f() { <std::process::Command>::new(\"x\"); }",
            "macro_rules! m { ($c:ty) => { $c::new(\"x\") }; }",
        ] {
            assert!(!one(source).is_empty(), "missed: {source}");
        }
    }

    #[test]
    fn the_scan_follows_an_alias_declared_in_another_file() {
        let files = [
            (
                "a.rs".to_string(),
                "pub type Cmd2 = Cmd; pub use std::process::Command as Cmd;".to_string(),
            ),
            (
                "b.rs".to_string(),
                "fn f() { Cmd::new(\"x\"); Cmd2::new(\"y\"); }".to_string(),
            ),
        ];
        let hits = spawns_in(&files);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert!(hits.iter().all(|(f, _)| f == "b.rs"));
    }

    #[test]
    fn only_test_only_cfgs_are_skipped() {
        let skipped = "use std::process::Command;
            #[cfg(test)] fn a() { Command::new(\"x\"); }
            #[cfg(all(test, unix))] fn b() { Command::new(\"x\"); }
            #[cfg(test)] mod m { fn c() { Command::new(\"x\"); } }";
        assert_eq!(one(skipped), Vec::<String>::new());
        for attr in [
            "#[cfg(any(test, unix))]",
            "#[cfg(any(test, feature = \"x\"))]",
            "#[cfg(not(test))]",
            "#[cfg(unix)]",
        ] {
            let source =
                format!("use std::process::Command; {attr} fn a() {{ Command::new(\"x\"); }}");
            assert!(!one(&source).is_empty(), "skipped: {attr}");
        }
    }

    #[test]
    fn the_scan_ignores_other_types() {
        let source = r#"
            struct Other;
            impl Other { fn new() -> Self { Other } }
            fn f() { let _ = Other::new(); let _ = scrubbed("ps"); }
        "#;
        assert_eq!(one(source), Vec::<String>::new());
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
            ("GDK_DPI_SCALE", "1.5"),
            ("GTK_THEME", "Adwaita:dark"),
            ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"),
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
            ("GTK_MODULES", "evil"),
            ("GTK_PATH", "/evil"),
            ("GTK_IM_MODULE_FILE", "/evil"),
            ("GTK_EXE_PREFIX", "/evil"),
            ("GDK_PIXBUF_MODULE_FILE", "/evil"),
            ("GDK_PIXBUF_MODULEDIR", "/evil"),
            ("WEBKIT_EXEC_PATH", "/evil"),
            ("WEBKIT_INJECTED_BUNDLE_PATH", "/evil"),
            ("WEBKIT_INSPECTOR_SERVER", "0.0.0.0:9222"),
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
