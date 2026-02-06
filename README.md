# NodeSpace

> **Faster context. Fewer tokens.**

AI coding assistants forget everything between sessions. NodeSpace gives them persistent, searchable access to your project knowledge — so you stop re-explaining your codebase every time you start a conversation.

**[nodespace.ai](https://nodespace.ai)** · **[Download](https://github.com/NodeSpaceAI/nodespace-core/releases)** · **[Discord](https://discord.gg/UHFZKzH9)**

[![NodeSpace Screenshot](docs/images/screenshot-alpha-preview.png)](docs/images/screenshot-alpha-preview.png)

> ⚠️ **Alpha Preview** — NodeSpace is in early development. Features may change and data formats are not yet stable.

---

## Why NodeSpace

Developers using AI assistants waste time copying files, re-explaining architecture, and watching context degrade mid-session. NodeSpace fixes this by sitting between your knowledge and your AI tools:

- **80% fewer roundtrips** — AI agents query your knowledge base via MCP instead of scanning files with grep/ripgrep
- **Runs entirely on your machine** — no cloud accounts, no API calls, no data leaving localhost
- **Works offline** — on planes, behind VPNs, anywhere

You write things down once. Every AI tool you use can find them instantly.

---

## Features

### 📅 Daily Journal
Quick access to today's context. One click opens a new entry for the current date, making it easy to capture thoughts throughout the day.

### 📝 Hierarchical Nodes
Create nested, indented blocks of content. Organize complex ideas with unlimited depth — like Logseq or Roam Research.

### 🏷️ Collections
Flexible organization that combines the best of folders and tags. A single node can belong to multiple collections without duplicating content.

### 🔗 @Mentions & Linking
Type `@` to link to any node. Build a knowledge graph by connecting related ideas. A backlinks panel shows everything that references each node.

### ✅ Task Management
Markdown-style tasks (`[ ]`, `[x]`, `[~]`) with visual checkboxes. Track progress on projects while keeping tasks connected to their context.

### 🔍 Semantic Search
Find what you mean, not just what you typed. Ask "Where do we handle authentication?" and find relevant content without exact keyword matches.

### 📂 Batch Import
Drop a folder of markdown files and import them in seconds. Semantic embeddings are generated in the background, so everything becomes searchable within minutes.

### 🤖 MCP Integration
Built-in MCP server for AI tools. Opens with the app — Claude Code, Cursor, Codex, and any MCP-compatible assistant can query your knowledge base locally.

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

> **Note:** The MCP server binds to `localhost` only and is accessible to other processes on your machine. Authentication is planned for a future release.

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

### Verify Connection

With NodeSpace running, test the connection:

```bash
curl -X POST http://localhost:3100/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
```

---

## Semantic Search

Once connected via MCP, your AI assistant can query your knowledge base semantically.

### Example Queries

Ask your AI assistant questions like:

- *"What is our development process for picking up issues?"*
- *"How do we handle authentication in the backend?"*
- *"What are the coding standards for this project?"*
- *"Find context related to the quarterly planning meeting"*

### Using the `search_semantic` Tool

AI tools can call the semantic search directly:

```json
{
  "name": "search_semantic",
  "arguments": {
    "query": "development process for implementing issues",
    "limit": 5
  }
}
```

This returns relevant nodes ranked by semantic similarity — not just keyword matches.

### Filtering by Collection

Narrow searches to specific areas:

```json
{
  "name": "search_semantic",
  "arguments": {
    "query": "validation flow",
    "collection": "Architecture"
  }
}
```

---

## Quick Start

1. **Open Daily Journal** — Click "Daily Journal" in the sidebar to start today's entry
2. **Create content** — Just start typing. Press `Enter` to create a new block below
3. **Organize with nesting** — Press `Tab` to indent a block under the one above. Press `Shift+Tab` to outdent
4. **Link your knowledge** — Type `@` to search and link to any other node
5. **Use Collections** — Expand "Collections" in the sidebar to organize content into categories
6. **Import existing docs** — Use File > Import to batch-import a folder of `.md` files. Hundreds of documents import in seconds; semantic indexing runs automatically in the background
7. **Connect your AI** — Configure MCP (see above) and ask your AI assistant to search your knowledge base

---

## Roadmap

| Feature | Status | Description |
|---------|--------|-------------|
| **Custom Node Types** | 🚧 In Progress | Define your own entity types with custom fields and behaviors |
| **Playbooks** | 📋 Planned | Installable workflow templates (ERP, Creator, Dev Team) |
| **Cloud Sync** | 📋 Planned | Real-time collaboration and cross-device sync |

See the [open issues](https://github.com/NodeSpaceAI/nodespace-core/issues) for the full backlog.

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
