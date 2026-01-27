#!/usr/bin/env bun
/**
 * Import docs folder into NodeSpace with proper titles and collections
 *
 * Usage:
 *   bun run scripts/import-docs.ts           # Full import
 *   bun run scripts/import-docs.ts --dry-run # Preview mappings without importing
 *
 * This script:
 * 1. Phase 1: Finds all .md files in docs/ and imports them
 *    - Derives collection path from folder structure with intelligent routing
 *    - Derives title from the first H1 heading in the file, or falls back to filename
 *    - Calls create_nodes_from_markdown MCP tool for each file
 *    - Builds a mapping of file paths to node IDs
 * 2. Phase 2: Resolves internal markdown links to nodespace:// URIs
 *    - Converts [text](./relative/path.md) to [text](nodespace://node-id)
 *    - Creates bidirectional mentions relationships for navigation
 * 3. Post-import: Updates archived docs to lifecycle_status: "archived"
 *
 * Collection Routing (Upper Case, No Prefix, Spaces Allowed):
 *   docs/architecture/decisions/foo.md -> "ADR"
 *   docs/path/to/archived/foo.md -> "Archived" (marked for lifecycle_status: "archived")
 *   docs/path/to/lessons/foo.md -> "Lessons"
 *   docs/troubleshooting/foo.md -> "Troubleshooting"
 *   docs/architecture/components/foo.md -> "Components"
 *   docs/architecture/business-logic/foo.md -> "Business Logic"
 *   docs/architecture/development/process/foo.md -> "Development:Process" (nested)
 *   docs/architecture/core/foo.md -> "Architecture:Core"
 *   docs/architecture/foo.md -> "Architecture"
 */

import { readdir, readFile } from "fs/promises";
import { join, relative, dirname, basename, resolve, normalize } from "path";

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

interface CollectionMetadata {
	collection: string;
	isArchived: boolean;
}

interface ImportResult {
	success: boolean;
	rootId?: string;
	error?: string;
	isArchived: boolean;
	filePath: string;
}

/** Mapping from relative file path (from docs root) to node ID */
type PathToNodeIdMap = Map<string, string>;

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
 * Convert path segment to Title Case with spaces
 * e.g., "business-logic" -> "Business Logic"
 */
function toTitleCase(segment: string): string {
	return segment
		.replace(/-/g, " ")
		.replace(/\b\w/g, (c) => c.toUpperCase());
}

/**
 * Derive collection path and metadata from file path
 * Returns collection path (Upper Case, spaces allowed) and whether it's archived
 *
 * Routing Rules:
 *   docs/architecture/decisions/foo.md -> "ADR"
 *   docs/path/to/archived/foo.md -> "Archived" (isArchived: true)
 *   docs/path/to/lessons/foo.md -> "Lessons"
 *   docs/troubleshooting/foo.md -> "Troubleshooting"
 *   docs/architecture/components/foo.md -> "Components"
 *   docs/architecture/business-logic/foo.md -> "Business Logic"
 *   docs/architecture/development/process/foo.md -> "Development:Process" (nested)
 *   docs/architecture/core/foo.md -> "Architecture:Core"
 *   docs/architecture/foo.md -> "Architecture"
 */
function deriveCollectionAndMetadata(filePath: string): CollectionMetadata {
	const relativePath = relative(DOCS_ROOT, filePath);
	const normalizedPath = relativePath.replace(/\\/g, "/").toLowerCase();
	const dirPath = dirname(relativePath);
	const segments = dirPath.replace(/\\/g, "/").split("/").filter(s => s && s !== ".");

	// Check for archived content anywhere in path
	const isArchived = normalizedPath.includes("/archived/") || segments.some(s => s.toLowerCase() === "archived");
	if (isArchived) {
		return { collection: "Archived", isArchived: true };
	}

	// Architecture Decision Records
	if (normalizedPath.includes("architecture/decisions/") || normalizedPath.startsWith("architecture/decisions")) {
		return { collection: "ADR", isArchived: false };
	}

	// Lessons learned (can be nested under various paths)
	if (normalizedPath.includes("/lessons/") || segments.some(s => s.toLowerCase() === "lessons")) {
		return { collection: "Lessons", isArchived: false };
	}

	// Troubleshooting
	if (normalizedPath.startsWith("troubleshooting/") || segments[0]?.toLowerCase() === "troubleshooting") {
		return { collection: "Troubleshooting", isArchived: false };
	}

	// Architecture-specific routing
	if (segments[0]?.toLowerCase() === "architecture") {
		const subPath = segments.slice(1);

		// Components
		if (subPath[0]?.toLowerCase() === "components") {
			return { collection: "Components", isArchived: false };
		}

		// Business Logic
		if (subPath[0]?.toLowerCase() === "business-logic") {
			return { collection: "Business Logic", isArchived: false };
		}

		// Development with nested structure
		if (subPath[0]?.toLowerCase() === "development") {
			const devSubPath = subPath.slice(1);
			if (devSubPath.length > 0) {
				const nestedCollection = devSubPath.map(s => toTitleCase(s)).join(":");
				return { collection: `Development:${nestedCollection}`, isArchived: false };
			}
			return { collection: "Development", isArchived: false };
		}

		// Core architecture docs
		if (subPath[0]?.toLowerCase() === "core") {
			return { collection: "Architecture:Core", isArchived: false };
		}

		// Other architecture subdirectories
		if (subPath.length > 0) {
			const archSubCollection = subPath.map(s => toTitleCase(s)).join(":");
			return { collection: `Architecture:${archSubCollection}`, isArchived: false };
		}

		// Root architecture docs
		return { collection: "Architecture", isArchived: false };
	}

	// Performance docs
	if (segments[0]?.toLowerCase() === "performance") {
		return { collection: "Performance", isArchived: false };
	}

	// Default: convert path to Title Case collection
	if (segments.length === 0) {
		return { collection: "Docs", isArchived: false };
	}

	const collection = segments.map(s => toTitleCase(s)).join(":");
	return { collection, isArchived: false };
}

/**
 * Extract all markdown links from content
 * Returns array of { fullMatch, linkText, linkPath }
 */
function extractMarkdownLinks(content: string): Array<{ fullMatch: string; linkText: string; linkPath: string }> {
	const linkPattern = /\[([^\]]+)\]\(([^)]+)\)/g;
	const links: Array<{ fullMatch: string; linkText: string; linkPath: string }> = [];
	let match;

	while ((match = linkPattern.exec(content)) !== null) {
		const [fullMatch, linkText, linkPath] = match;
		// Only process relative markdown links (not http/https, nodespace://, or anchors)
		if (
			linkPath.endsWith(".md") &&
			!linkPath.startsWith("http://") &&
			!linkPath.startsWith("https://") &&
			!linkPath.startsWith("nodespace://") &&
			!linkPath.startsWith("#")
		) {
			links.push({ fullMatch, linkText, linkPath });
		}
	}

	return links;
}

/**
 * Resolve a relative link path to an absolute path relative to docs root
 * Example: from "architecture/core/system-overview.md" with link "../decisions/foo.md"
 *          returns "architecture/decisions/foo.md"
 */
function resolveRelativePath(fromFile: string, linkPath: string): string {
	// Get the directory of the source file
	const fromDir = dirname(fromFile);
	// Resolve the link path relative to that directory
	const resolved = resolve(fromDir, linkPath);
	// Normalize to remove any . or .. segments
	return normalize(resolved).replace(/\\/g, "/");
}

/**
 * Convert markdown links to nodespace:// URIs
 * Returns the updated content and a list of resolved links for reporting
 */
function convertLinksToNodespaceUris(
	content: string,
	sourceFile: string,
	pathToNodeId: PathToNodeIdMap
): { updatedContent: string; resolvedLinks: number; unresolvedLinks: string[] } {
	const links = extractMarkdownLinks(content);
	let updatedContent = content;
	let resolvedLinks = 0;
	const unresolvedLinks: string[] = [];

	for (const { fullMatch, linkText, linkPath } of links) {
		// Resolve the relative path to get the target file path
		const targetPath = resolveRelativePath(sourceFile, linkPath);

		// Look up the node ID for this path
		const nodeId = pathToNodeId.get(targetPath);

		if (nodeId) {
			// Replace with nodespace:// URI
			const newLink = `[${linkText}](nodespace://${nodeId})`;
			updatedContent = updatedContent.replace(fullMatch, newLink);
			resolvedLinks++;
		} else {
			// Track unresolved links for reporting
			unresolvedLinks.push(`${linkPath} → ${targetPath}`);
		}
	}

	return { updatedContent, resolvedLinks, unresolvedLinks };
}

/**
 * Get node content by ID via MCP
 */
async function getNodeContent(nodeId: string): Promise<{ content: string; version: number } | null> {
	try {
		const result = (await callMCP("tools/call", {
			name: "get_node",
			arguments: { node_id: nodeId },
		})) as { content: Array<{ text: string }> };

		const resultText = result?.content?.[0]?.text || "{}";
		const parsed = JSON.parse(resultText);

		if (parsed.content !== undefined && parsed.version !== undefined) {
			return { content: parsed.content, version: parsed.version };
		}
		return null;
	} catch {
		return null;
	}
}

/**
 * Update node content via MCP
 */
async function updateNodeContent(nodeId: string, content: string): Promise<boolean> {
	try {
		await callMCP("tools/call", {
			name: "update_node",
			arguments: {
				node_id: nodeId,
				content: content,
			},
		});
		return true;
	} catch (error) {
		console.error(`    ✗ Failed to update ${nodeId}:`, error);
		return false;
	}
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
	total: number,
	dryRun: boolean
): Promise<ImportResult> {
	const relativePath = relative(DOCS_ROOT, filePath);
	const { collection, isArchived } = deriveCollectionAndMetadata(filePath);

	try {
		const content = await readFile(filePath, "utf-8");
		const titlePreview = extractTitlePreview(content, filePath);

		console.log(`[${index + 1}/${total}] ${dryRun ? "[DRY-RUN] " : ""}${relativePath}`);
		console.log(`    Title: ${titlePreview}`);
		console.log(`    Collection: ${collection}`);
		if (isArchived) {
			console.log(`    ⚠️ Will be marked as ARCHIVED`);
		}

		if (dryRun) {
			console.log(`    → Would import to collection "${collection}"\n`);
			return { success: true, isArchived, filePath: relativePath };
		}

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
			return { success: true, rootId: parsed.root_id, isArchived, filePath: relativePath };
		} else {
			console.log(`    ✗ No root_id in response\n`);
			return { success: false, error: "No root_id in response", isArchived, filePath: relativePath };
		}
	} catch (error) {
		const errorMsg = error instanceof Error ? error.message : String(error);
		console.log(`    ✗ Error: ${errorMsg}\n`);
		return { success: false, error: errorMsg, isArchived, filePath: relativePath };
	}
}

/**
 * Update a node's lifecycle_status to "archived"
 */
async function archiveNode(nodeId: string): Promise<boolean> {
	try {
		await callMCP("tools/call", {
			name: "update_node",
			arguments: {
				node_id: nodeId,
				lifecycle_status: "archived",
			},
		});
		return true;
	} catch (error) {
		console.error(`    ✗ Failed to archive ${nodeId}:`, error);
		return false;
	}
}

/**
 * Main import function
 */
async function main() {
	const args = process.argv.slice(2);
	const dryRun = args.includes("--dry-run");

	console.log("=== NodeSpace Docs Import ===\n");
	console.log(`Docs root: ${DOCS_ROOT}`);
	if (dryRun) {
		console.log("Mode: DRY-RUN (no changes will be made)\n");
	} else {
		console.log("Mode: FULL IMPORT\n");
	}

	// Initialize MCP (skip for dry-run)
	if (!dryRun) {
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
	}

	// Find all markdown files
	console.log("Scanning for markdown files...");
	const files = await findMarkdownFiles(DOCS_ROOT);
	console.log(`Found ${files.length} markdown files\n`);

	if (files.length === 0) {
		console.log("No files to import.");
		return;
	}

	// Collection statistics for dry-run
	if (dryRun) {
		const collectionStats = new Map<string, number>();
		let archivedCount = 0;

		for (const file of files) {
			const { collection, isArchived } = deriveCollectionAndMetadata(file);
			collectionStats.set(collection, (collectionStats.get(collection) || 0) + 1);
			if (isArchived) archivedCount++;
		}

		console.log("=== Collection Distribution ===");
		const sortedCollections = [...collectionStats.entries()].sort((a, b) => b[1] - a[1]);
		for (const [collection, count] of sortedCollections) {
			const archivedIndicator = collection === "Archived" ? " 📦" : "";
			console.log(`  ${collection}: ${count} files${archivedIndicator}`);
		}
		console.log(`\nTotal archived: ${archivedCount} files`);
		console.log("\n=== File Mappings ===\n");
	}

	// ============================================================
	// PHASE 1: Import all files and build path-to-nodeId mapping
	// ============================================================
	console.log("=== Phase 1: Importing Documents ===\n");

	let successCount = 0;
	let errorCount = 0;
	const errors: Array<{ file: string; error: string }> = [];
	const archivedRootIds: string[] = [];
	const pathToNodeId: PathToNodeIdMap = new Map();
	const importedResults: ImportResult[] = [];

	for (let i = 0; i < files.length; i++) {
		const result = await importFile(files[i], i, files.length, dryRun);
		importedResults.push(result);

		if (result.success) {
			successCount++;
			// Build the path-to-nodeId mapping
			if (result.rootId && result.filePath) {
				pathToNodeId.set(result.filePath, result.rootId);
			}
			// Track archived nodes for post-import lifecycle update
			if (result.isArchived && result.rootId) {
				archivedRootIds.push(result.rootId);
			}
		} else {
			errorCount++;
			errors.push({
				file: relative(DOCS_ROOT, files[i]),
				error: result.error || "Unknown error",
			});
		}

		// Small delay to avoid overwhelming the server (only for actual imports)
		if (!dryRun) {
			await new Promise((resolve) => setTimeout(resolve, 100));
		}
	}

	console.log(`Phase 1 complete: ${successCount} imported, ${errorCount} failed\n`);

	// ============================================================
	// PHASE 2: Resolve internal markdown links to nodespace:// URIs
	// ============================================================
	if (!dryRun && pathToNodeId.size > 0) {
		console.log("=== Phase 2: Resolving Internal Links ===\n");

		let linksResolved = 0;
		let docsUpdated = 0;
		const allUnresolvedLinks: Array<{ file: string; links: string[] }> = [];

		for (const result of importedResults) {
			if (!result.success || !result.rootId || !result.filePath) continue;

			// Read the original file content to find links
			const fullPath = join(DOCS_ROOT, result.filePath);
			const originalContent = await readFile(fullPath, "utf-8");

			// Check if there are any markdown links to convert
			const links = extractMarkdownLinks(originalContent);
			if (links.length === 0) continue;

			// Get current node content (which may differ slightly from original due to markdown parsing)
			const nodeData = await getNodeContent(result.rootId);
			if (!nodeData) {
				console.log(`  ⚠️ Could not fetch ${result.filePath} (${result.rootId})`);
				continue;
			}

			// Convert links in the node content
			const { updatedContent, resolvedLinks, unresolvedLinks } = convertLinksToNodespaceUris(
				nodeData.content,
				result.filePath,
				pathToNodeId
			);

			if (resolvedLinks > 0) {
				// Update the node with converted links
				if (await updateNodeContent(result.rootId, updatedContent)) {
					console.log(`  ✓ ${result.filePath}: ${resolvedLinks} links resolved`);
					linksResolved += resolvedLinks;
					docsUpdated++;
				}
			}

			if (unresolvedLinks.length > 0) {
				allUnresolvedLinks.push({ file: result.filePath, links: unresolvedLinks });
			}
		}

		console.log(`\nPhase 2 complete: ${linksResolved} links resolved in ${docsUpdated} documents`);

		if (allUnresolvedLinks.length > 0) {
			console.log(`\n⚠️ Unresolved links (target files not imported):`);
			for (const { file, links } of allUnresolvedLinks.slice(0, 10)) {
				console.log(`  ${file}:`);
				for (const link of links.slice(0, 3)) {
					console.log(`    - ${link}`);
				}
				if (links.length > 3) {
					console.log(`    ... and ${links.length - 3} more`);
				}
			}
			if (allUnresolvedLinks.length > 10) {
				console.log(`  ... and ${allUnresolvedLinks.length - 10} more files with unresolved links`);
			}
		}
		console.log("");
	}

	// Post-import: Update lifecycle_status for archived docs
	if (!dryRun && archivedRootIds.length > 0) {
		console.log("\n=== Marking Archived Documents ===");
		console.log(`Updating ${archivedRootIds.length} documents to lifecycle_status: "archived"\n`);

		let archiveSuccessCount = 0;
		for (const rootId of archivedRootIds) {
			console.log(`  Archiving: ${rootId}`);
			if (await archiveNode(rootId)) {
				archiveSuccessCount++;
				console.log(`    ✓ Archived\n`);
			}
		}
		console.log(`Archived ${archiveSuccessCount}/${archivedRootIds.length} documents`);
	}

	// Summary
	console.log("\n=== Import Summary ===");
	console.log(`Total files: ${files.length}`);
	console.log(`Successful: ${successCount}`);
	console.log(`Failed: ${errorCount}`);
	if (archivedRootIds.length > 0) {
		console.log(`Archived: ${archivedRootIds.length}`);
	}

	if (errors.length > 0) {
		console.log("\nFailed files:");
		for (const { file, error } of errors) {
			console.log(`  - ${file}: ${error}`);
		}
	}

	if (dryRun) {
		console.log("\n💡 Run without --dry-run to perform actual import.");
	}

	console.log("\nDone!");
}

// Run the import
main().catch((error) => {
	console.error("Fatal error:", error);
	process.exit(1);
});
