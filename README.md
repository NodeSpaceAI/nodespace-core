# NodeSpace

> **Faster context. Fewer tokens.**

AI coding assistants forget everything between sessions. NodeSpace gives them persistent, searchable access to your project knowledge — so you stop re-explaining your codebase every time you start a conversation.

**[nodespace.ai](https://nodespace.ai)** · **[Download](https://github.com/NodeSpaceAI/nodespace-core/releases)** · **[Discord](https://discord.gg/UHFZKzH9)**

> ⚠️ **Alpha Preview** — NodeSpace is in early development. Features may change and data formats are not yet stable.

![NodeSpace Screenshot](assets/screenshot-alpha-preview.png)

---

## Why NodeSpace

Developers using AI assistants waste time copying files, re-explaining architecture, and watching context degrade mid-session. NodeSpace fixes this by sitting between your knowledge and your AI tools:

- **80% fewer roundtrips** — AI agents query your knowledge base via the NodeSpace skill instead of scanning files with grep/ripgrep
- **Runs entirely on your machine** — no cloud accounts, no API calls, no data leaving localhost
- **Works offline** — on planes, behind VPNs, anywhere

You write things down once. Every AI tool you use can find them instantly.

---

## Installation

### macOS (Apple Silicon) — Homebrew

```bash
brew install --cask nodespaceai/nodespace/nodespace
```

Apple Silicon is the only supported macOS target. The cask installs a signed and notarized build — no Gatekeeper prompt to work around.

### Other platforms — manual download

**[Download NodeSpace →](https://github.com/NodeSpaceAI/nodespace-core/releases)**

| Platform | Format |
|----------|--------|
| macOS 14 (Sonoma) or later, Apple Silicon | `.dmg` |
| Windows | `.msi` or `.exe` |
| Linux | `nodespace`/`nodespaced` binaries (CLI + daemon only — no packaged desktop app yet) |

### Team collaboration

For team collaboration, contact [developer@nodespace.ai](mailto:developer@nodespace.ai).

### Build from Source

**Prerequisites:**
- [Bun 1.0+](https://bun.sh) — `curl -fsSL https://bun.sh/install | bash`
- [rustup](https://rustup.rs) — `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`. It installs the Rust toolchain `rust-toolchain.toml` pins on the first build.

```bash
git clone https://github.com/NodeSpaceAI/nodespace-core
cd nodespace-core
bun install
bun run tauri:dev
```

---

## Running Tests

```bash
bun run test          # Fast unit tests (Happy-DOM)
bun run test:browser  # Browser integration tests (Playwright)
bun run test:all      # All tests (unit + browser + Rust)
bun run rust:test     # Rust backend tests only
```

---

## Community

- 💬 [Join our Discord](https://discord.gg/UHFZKzH9) — ask questions, share feedback, follow development
- 🌟 [Star this repo](https://github.com/NodeSpaceAI/nodespace-core) if NodeSpace is useful to you
- 🐛 [Report a bug](https://github.com/NodeSpaceAI/nodespace-core/issues/new)

---

## License

NodeSpace is licensed under the [Functional Source License 1.1 (Apache 2.0)](https://fsl.software/).

- ✅ Use NodeSpace freely for any purpose
- ✅ Modify the code to fit your needs

See [LICENSE](LICENSE) for the full text.
