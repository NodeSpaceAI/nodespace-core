# NodeSpace

> **Faster context. Fewer tokens.**

AI coding assistants forget everything between sessions. NodeSpace gives them persistent, searchable access to your project knowledge — so you stop re-explaining your codebase every time you start a conversation.

**[nodespace.ai](https://nodespace.ai)** · **[Download](https://github.com/NodeSpaceAI/nodespace-core/releases)** · **[Discord](https://discord.gg/UHFZKzH9)**

> ⚠️ **Alpha Preview** — NodeSpace is in early development. Features may change and data formats are not yet stable.

---

## Documentation

For full documentation, see [nodespace-docs](../nodespace-docs/).

---

## Why NodeSpace

Developers using AI assistants waste time copying files, re-explaining architecture, and watching context degrade mid-session. NodeSpace fixes this by sitting between your knowledge and your AI tools:

- **80% fewer roundtrips** — AI agents query your knowledge base via MCP instead of scanning files with grep/ripgrep
- **Runs entirely on your machine** — no cloud accounts, no API calls, no data leaving localhost
- **Works offline** — on planes, behind VPNs, anywhere

You write things down once. Every AI tool you use can find them instantly.

---

## Installation

### Download the Desktop App

**[Download NodeSpace →](https://github.com/NodeSpaceAI/nodespace-core/releases)**

| Platform | Format |
|----------|--------|
| macOS (Apple Silicon) | `.dmg` |
| Windows | `.exe` or `.msi` |

### Build from Source

**Prerequisites:**
- [Bun 1.0+](https://bun.sh) — `curl -fsSL https://bun.sh/install | bash`
- [Rust 1.80+](https://rustup.rs) — `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

```bash
git clone https://github.com/NodeSpaceAI/nodespace-core
cd nodespace-core
bun install
bun run tauri:dev
```

---

## MCP Setup

NodeSpace includes a built-in MCP server that starts automatically when you open the app. Your AI tools connect to it locally — there's nothing to deploy or host.

### Claude Code / Cursor / Codex / Other MCP Clients

Add to your MCP settings (e.g., `~/.claude.json` for Claude Code):

```json
{
  "mcpServers": {
    "nodespace": {
      "type": "http",
      "url": "http://localhost:3100/mcp"
    }
  }
}
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
