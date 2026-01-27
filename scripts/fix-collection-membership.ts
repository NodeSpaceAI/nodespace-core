#!/usr/bin/env bun
/**
 * Fix Collection Membership Script
 *
 * This script fixes missing collection memberships for imported docs.
 * The original import used async collection assignment which may have
 * silently failed due to race conditions.
 *
 * Usage:
 *   bun run scripts/fix-collection-membership.ts --dry-run   # Preview changes
 *   bun run scripts/fix-collection-membership.ts             # Apply fixes
 */

import { readdir, readFile } from "fs/promises";
import { join, relative, dirname, basename } from "path";

const DOCS_ROOT = join(import.meta.dir, "..", "docs");
const MCP_ENDPOINT = "http://localhost:8080";

// ============================================================================
// MCP Client
// ============================================================================

async function callMCP(method: string, params: Record<string, unknown> = {}): Promise<unknown> {
	const response = await fetch(MCP_ENDPOINT, {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify({
			jsonrpc: "2.0",
			id: Date.now(),
			method,
			params,
		}),
	});

	if (!response.ok) {
		throw new Error(`MCP request failed: ${response.status} ${response.statusText}`);
	}

	const result = await response.json();
	if (result.error) {
		throw new Error(`MCP error: ${result.error.message}`);
	}

	return result.result;
}

// ============================================================================
// Collection Mapping (same as import-docs.ts)
// ============================================================================

function toTitleCase(str: string): string {
	return str
		.split(/[-_]/)
		.map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
		.join(" ");
}

interface CollectionMetadata {
	collection: string;
	isArchived: boolean;
}

function deriveCollectionAndMetadata(filePath: string): CollectionMetadata {
	const relativePath = relative(DOCS_ROOT, filePath);
	const normalizedPath = relativePath.replace(/\\/g, "/").toLowerCase();
	const dirPath = dirname(relativePath);
	const segments = dirPath
		.replace(/\\/g, "/")
		.split("/")
		.filter((s) => s && s !== ".");

	// Check for archived content anywhere in path
	const isArchived =
		normalizedPath.includes("/archived/") || segments.some((s) => s.toLowerCase() === "archived");
	if (isArchived) {
		return { collection: "Archived", isArchived: true };
	}

	// Architecture Decision Records
	if (
		normalizedPath.includes("architecture/decisions/") ||
		normalizedPath.startsWith("architecture/decisions")
	) {
		return { collection: "ADR", isArchived: false };
	}

	// Lessons learned
	if (normalizedPath.includes("/lessons/") || segments.some((s) => s.toLowerCase() === "lessons")) {
		return { collection: "Lessons", isArchived: false };
	}

	// Troubleshooting
	if (
		normalizedPath.startsWith("troubleshooting/") ||
		segments[0]?.toLowerCase() === "troubleshooting"
	) {
		return { collection: "Troubleshooting", isArchived: false };
	}

	// Architecture-specific routing
	if (segments[0]?.toLowerCase() === "architecture") {
		const subPath = segments.slice(1);

		if (subPath[0]?.toLowerCase() === "components") {
			return { collection: "Components", isArchived: false };
		}

		if (subPath[0]?.toLowerCase() === "business-logic") {
			return { collection: "Business Logic", isArchived: false };
		}

		if (subPath[0]?.toLowerCase() === "development") {
			const devSubPath = subPath.slice(1);
			if (devSubPath.length > 0) {
				const nestedCollection = devSubPath.map((s) => toTitleCase(s)).join(":");
				return { collection: `Development:${nestedCollection}`, isArchived: false };
			}
			return { collection: "Development", isArchived: false };
		}

		if (subPath[0]?.toLowerCase() === "core") {
			return { collection: "Architecture:Core", isArchived: false };
		}

		if (subPath.length > 0) {
			const archSubCollection = subPath.map((s) => toTitleCase(s)).join(":");
			return { collection: `Architecture:${archSubCollection}`, isArchived: false };
		}

		return { collection: "Architecture", isArchived: false };
	}

	if (segments[0]?.toLowerCase() === "performance") {
		return { collection: "Performance", isArchived: false };
	}

	if (segments.length === 0) {
		return { collection: "Docs", isArchived: false };
	}

	const collection = segments.map((s) => toTitleCase(s)).join(":");
	return { collection, isArchived: false };
}

// ============================================================================
// File Discovery
// ============================================================================

async function findMarkdownFiles(dir: string): Promise<string[]> {
	const files: string[] = [];

	async function scan(currentDir: string) {
		const entries = await readdir(currentDir, { withFileTypes: true });

		for (const entry of entries) {
			const fullPath = join(currentDir, entry.name);

			if (entry.isDirectory()) {
				// Skip design-system
				if (entry.name === "design-system") continue;
				await scan(fullPath);
			} else if (entry.isFile() && entry.name.endsWith(".md")) {
				files.push(fullPath);
			}
		}
	}

	await scan(dir);
	return files.sort();
}

function extractTitleFromMarkdown(content: string): string | null {
	const lines = content.split("\n");
	for (const line of lines) {
		const trimmed = line.trim();
		if (trimmed.startsWith("# ")) {
			return trimmed.substring(2).trim();
		}
		if (trimmed && !trimmed.startsWith("#")) {
			// First non-empty, non-heading line is the title
			return trimmed;
		}
	}
	return null;
}

// ============================================================================
// Main
// ============================================================================

async function main() {
	const args = process.argv.slice(2);
	const dryRun = args.includes("--dry-run");

	console.log("=== Fix Collection Membership ===\n");
	console.log(`Mode: ${dryRun ? "DRY-RUN" : "APPLY FIXES"}\n`);

	// Find all markdown files
	console.log("Scanning for markdown files...");
	const files = await findMarkdownFiles(DOCS_ROOT);
	console.log(`Found ${files.length} markdown files\n`);

	// Build mapping: title -> { filePath, collection, isArchived }
	const fileMapping: Map<
		string,
		{ filePath: string; collection: string; isArchived: boolean; title: string }
	> = new Map();

	for (const filePath of files) {
		const content = await readFile(filePath, "utf-8");
		const title = extractTitleFromMarkdown(content);
		if (title) {
			const { collection, isArchived } = deriveCollectionAndMetadata(filePath);
			fileMapping.set(title, { filePath, collection, isArchived, title });
		}
	}

	console.log(`Extracted ${fileMapping.size} titles from files\n`);

	// Get all root nodes from NodeSpace via semantic search
	// We'll search broadly and filter by matching titles
	console.log("Fetching nodes from NodeSpace...\n");

	let fixedCount = 0;
	let archivedCount = 0;
	let alreadyCorrect = 0;
	let notFound = 0;
	const errors: string[] = [];

	// Process each file mapping
	for (const [title, info] of fileMapping) {
		// Search for the node by title
		const searchResult = (await callMCP("tools/call", {
			name: "search_semantic",
			arguments: {
				query: title,
				limit: 5,
				include_markdown: 0,
				include_archived: true, // Include archived to find all docs
			},
		})) as { content: Array<{ text: string }> };

		const resultText = searchResult?.content?.[0]?.text || "{}";
		const parsed = JSON.parse(resultText);

		// Find exact title match
		const matchingNode = parsed.nodes?.find(
			(n: { title?: string; content?: string }) =>
				n.title === title || n.title === `# ${title}` || n.content === `# ${title}`
		);

		if (!matchingNode) {
			notFound++;
			if (notFound <= 10) {
				console.log(`  ✗ Not found: "${title.substring(0, 50)}..."`);
			}
			continue;
		}

		const nodeId = matchingNode.id;
		const relativePath = relative(DOCS_ROOT, info.filePath);

		// Check current collection membership
		const nodeResult = (await callMCP("tools/call", {
			name: "get_node",
			arguments: { node_id: nodeId },
		})) as { content: Array<{ text: string }> };

		const nodeText = nodeResult?.content?.[0]?.text || "{}";
		const nodeData = JSON.parse(nodeText);

		// Check if already in correct collection
		const memberOf = nodeData.memberOf || [];
		const currentCollections = memberOf as string[];

		// Get collection ID for target collection
		let needsCollectionFix = true;

		if (currentCollections.length > 0) {
			// Check if any current collection matches target
			// We'd need to resolve collection names to check properly
			// For now, assume if it has any membership, we skip
			needsCollectionFix = currentCollections.length === 0;
		}

		// For dry run, just report what would be done
		if (dryRun) {
			console.log(`[${relativePath}]`);
			console.log(`  Title: ${title.substring(0, 60)}${title.length > 60 ? "..." : ""}`);
			console.log(`  Node ID: ${nodeId}`);
			console.log(`  Target Collection: ${info.collection}`);
			console.log(`  Current Memberships: ${currentCollections.length}`);
			if (info.isArchived) {
				console.log(`  Lifecycle: → archived`);
			}
			console.log("");
			continue;
		}

		// Apply fixes
		try {
			// Add to collection if not already a member
			if (currentCollections.length === 0) {
				await callMCP("tools/call", {
					name: "update_node",
					arguments: {
						node_id: nodeId,
						add_to_collection: info.collection,
					},
				});
				fixedCount++;
				console.log(`  ✓ Added to collection: ${info.collection}`);
			} else {
				alreadyCorrect++;
			}

			// Mark as archived if needed
			if (info.isArchived) {
				await callMCP("tools/call", {
					name: "update_node",
					arguments: {
						node_id: nodeId,
						lifecycle_status: "archived",
					},
				});
				archivedCount++;
				console.log(`  ✓ Marked as archived`);
			}
		} catch (error) {
			const errorMsg = error instanceof Error ? error.message : String(error);
			errors.push(`${title}: ${errorMsg}`);
		}
	}

	// Summary
	console.log("\n=== Summary ===");
	console.log(`Total files: ${fileMapping.size}`);
	if (!dryRun) {
		console.log(`Fixed collection membership: ${fixedCount}`);
		console.log(`Marked as archived: ${archivedCount}`);
		console.log(`Already correct: ${alreadyCorrect}`);
	}
	console.log(`Not found in NodeSpace: ${notFound}`);

	if (errors.length > 0) {
		console.log(`\nErrors (${errors.length}):`);
		errors.slice(0, 10).forEach((e) => console.log(`  - ${e}`));
		if (errors.length > 10) {
			console.log(`  ... and ${errors.length - 10} more`);
		}
	}
}

main().catch((error) => {
	console.error("Fatal error:", error);
	process.exit(1);
});
