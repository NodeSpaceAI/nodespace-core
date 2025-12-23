#!/usr/bin/env bun
/**
 * Import docs folder into NodeSpace with proper titles and collections
 *
 * Usage:
 *   bun run scripts/import-docs.ts
 *
 * This script:
 * 1. Finds all .md files in docs/
 * 2. Derives collection path from folder structure (e.g., docs/architecture/core/file.md -> "architecture:core")
 * 3. Derives title from the first H1 heading in the file, or falls back to filename
 * 4. Calls create_nodes_from_markdown MCP tool for each file
 */

import { readdir, readFile, stat } from "fs/promises";
import { join, relative, dirname, basename } from "path";

const DOCS_ROOT = join(import.meta.dir, "..", "docs");
const MCP_ENDPOINT = "http://localhost:3100/mcp"; // NodeSpace MCP HTTP endpoint

interface MCPRequest {
	jsonrpc: "2.0";
	id: number;
	method: string;
	params: Record<string, unknown>;
}

interface MCPResponse {
	jsonrpc: "2.0";
	id: number;
	result?: unknown;
	error?: { code: number; message: string };
}

let requestId = 1;

/**
 * Call an MCP tool via HTTP
 */
async function callMCP(
	method: string,
	params: Record<string, unknown>
): Promise<unknown> {
	const request: MCPRequest = {
		jsonrpc: "2.0",
		id: requestId++,
		method,
		params,
	};

	const response = await fetch(MCP_ENDPOINT, {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify(request),
	});

	if (!response.ok) {
		throw new Error(`HTTP error: ${response.status} ${response.statusText}`);
	}

	const result = (await response.json()) as MCPResponse;

	if (result.error) {
		throw new Error(`MCP error: ${result.error.message}`);
	}

	return result.result;
}

/**
 * Initialize MCP connection
 */
async function initializeMCP(): Promise<void> {
	console.log("Initializing MCP connection...");
	await callMCP("initialize", {
		protocolVersion: "2024-11-05",
		clientInfo: { name: "import-docs-script", version: "1.0.0" },
	});
	console.log("MCP initialized successfully\n");
}

/**
 * Extract title preview from markdown content (first H1 heading) for logging
 */
function extractTitlePreview(content: string, filename: string): string {
	const lines = content.split("\n");
	for (const line of lines) {
		const h1Match = line.match(/^#\s+(.+)$/);
		if (h1Match) {
			return h1Match[1].trim();
		}
	}
	// Fallback to filename without extension
	return basename(filename, ".md")
		.replace(/-/g, " ")
		.replace(/\b\w/g, (c) => c.toUpperCase());
}

/**
 * Derive collection path from file path
 * e.g., docs/architecture/core/file.md -> "architecture:core"
 */
function deriveCollection(filePath: string): string {
	const relativePath = relative(DOCS_ROOT, filePath);
	const dirPath = dirname(relativePath);

	if (dirPath === "." || dirPath === "") {
		return "docs"; // Root level docs
	}

	// Convert path separators to collection delimiters
	return dirPath.replace(/\//g, ":").replace(/\\/g, ":");
}

/**
 * Find all markdown files recursively
 */
async function findMarkdownFiles(dir: string): Promise<string[]> {
	const files: string[] = [];
	const entries = await readdir(dir, { withFileTypes: true });

	for (const entry of entries) {
		const fullPath = join(dir, entry.name);

		if (entry.isDirectory()) {
			// Skip hidden directories and design-system
			if (!entry.name.startsWith(".") && entry.name !== "design-system") {
				files.push(...(await findMarkdownFiles(fullPath)));
			}
		} else if (entry.isFile() && entry.name.endsWith(".md")) {
			files.push(fullPath);
		}
	}

	return files;
}

/**
 * Import a single markdown file
 */
async function importFile(
	filePath: string,
	index: number,
	total: number
): Promise<{ success: boolean; rootId?: string; error?: string }> {
	const relativePath = relative(DOCS_ROOT, filePath);
	const collection = deriveCollection(filePath);

	try {
		const content = await readFile(filePath, "utf-8");
		const titlePreview = extractTitlePreview(content, filePath);

		console.log(`[${index + 1}/${total}] Importing: ${relativePath}`);
		console.log(`    Title: ${titlePreview}`);
		console.log(`    Collection: ${collection}`);

		// Don't pass title - let the handler extract it from the first line
		const result = (await callMCP("tools/call", {
			name: "create_nodes_from_markdown",
			arguments: {
				markdown_content: content,
				collection: collection,
				sync_import: false, // Fire-and-forget for faster imports
			},
		})) as { content: Array<{ text: string }> };

		// Parse the result
		const resultText = result?.content?.[0]?.text || "{}";
		const parsed = JSON.parse(resultText);

		if (parsed.root_id) {
			console.log(`    ✓ Created root: ${parsed.root_id}\n`);
			return { success: true, rootId: parsed.root_id };
		} else {
			console.log(`    ✗ No root_id in response\n`);
			return { success: false, error: "No root_id in response" };
		}
	} catch (error) {
		const errorMsg = error instanceof Error ? error.message : String(error);
		console.log(`    ✗ Error: ${errorMsg}\n`);
		return { success: false, error: errorMsg };
	}
}

/**
 * Main import function
 */
async function main() {
	console.log("=== NodeSpace Docs Import ===\n");
	console.log(`Docs root: ${DOCS_ROOT}\n`);

	// Initialize MCP
	try {
		await initializeMCP();
	} catch (error) {
		console.error(
			"Failed to connect to NodeSpace MCP server at",
			MCP_ENDPOINT
		);
		console.error("Make sure NodeSpace is running with HTTP MCP enabled.");
		console.error("Error:", error instanceof Error ? error.message : error);
		process.exit(1);
	}

	// Find all markdown files
	console.log("Scanning for markdown files...");
	const files = await findMarkdownFiles(DOCS_ROOT);
	console.log(`Found ${files.length} markdown files\n`);

	if (files.length === 0) {
		console.log("No files to import.");
		return;
	}

	// Import each file
	let successCount = 0;
	let errorCount = 0;
	const errors: Array<{ file: string; error: string }> = [];

	for (let i = 0; i < files.length; i++) {
		const result = await importFile(files[i], i, files.length);
		if (result.success) {
			successCount++;
		} else {
			errorCount++;
			errors.push({
				file: relative(DOCS_ROOT, files[i]),
				error: result.error || "Unknown error",
			});
		}

		// Small delay to avoid overwhelming the server
		await new Promise((resolve) => setTimeout(resolve, 100));
	}

	// Summary
	console.log("\n=== Import Summary ===");
	console.log(`Total files: ${files.length}`);
	console.log(`Successful: ${successCount}`);
	console.log(`Failed: ${errorCount}`);

	if (errors.length > 0) {
		console.log("\nFailed files:");
		for (const { file, error } of errors) {
			console.log(`  - ${file}: ${error}`);
		}
	}

	console.log("\nDone!");
}

// Run the import
main().catch((error) => {
	console.error("Fatal error:", error);
	process.exit(1);
});
