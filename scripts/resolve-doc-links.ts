#!/usr/bin/env bun
/**
 * Resolve internal markdown links to nodespace:// URIs (Phase 2)
 *
 * Approach:
 * 1. Build file path -> title mapping from local docs
 * 2. Build title -> nodeId mapping by querying NodeSpace
 * 3. For each file with links, find its nodes and update content
 */

import { readFile } from "fs/promises";
import { join, relative, dirname, resolve, normalize } from "path";
import { normalize as normalizePath } from "node:path";
import { $ } from "bun";

const DOCS_ROOT = join(import.meta.dir, "..", "docs");
const MCP_ENDPOINT = "http://localhost:3100/mcp";

let requestId = 1;

async function callMCP(method: string, params: Record<string, unknown>): Promise<unknown> {
	const response = await fetch(MCP_ENDPOINT, {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify({ jsonrpc: "2.0", id: requestId++, method, params })
	});
	const result = await response.json() as { result?: unknown; error?: { message: string } };
	if (result.error) throw new Error(result.error.message);
	return result.result;
}

function extractTitle(content: string): string | null {
	const match = content.match(/^#\s+(.+)$/m);
	return match ? match[1].trim() : null;
}

function extractLinks(content: string): Array<{ full: string; text: string; path: string }> {
	const pattern = /\[([^\]]+)\]\(([^)]+\.md)\)/g;
	const links: Array<{ full: string; text: string; path: string }> = [];
	let match;
	while ((match = pattern.exec(content)) !== null) {
		const [full, text, path] = match;
		if (!path.startsWith("http") && !path.startsWith("nodespace://")) {
			links.push({ full, text, path });
		}
	}
	return links;
}

function resolvePath(fromFile: string, linkPath: string): string {
	const fromDir = dirname(fromFile);
	const absolutePath = resolve(fromDir, linkPath);
	// Return relative to current dir (which is the fromFile's base)
	return normalize(absolutePath).replace(/\\/g, "/");
}

function resolveToRelative(fromFile: string, linkPath: string): string {
	// fromFile is relative to DOCS_ROOT, linkPath is relative to fromFile
	const fromDir = dirname(fromFile);
	// Join and normalize
	const joined = join(fromDir, linkPath);
	return normalize(joined).replace(/\\/g, "/");
}

async function main() {
	const dryRun = process.argv.includes("--dry-run");
	console.log(`=== Link Resolution (Phase 2) ${dryRun ? "[DRY-RUN]" : ""} ===\n`);

	// Initialize MCP
	console.log("Initializing MCP...");
	await callMCP("initialize", {
		protocolVersion: "2024-11-05",
		clientInfo: { name: "link-resolver", version: "1.0" }
	});

	// Step 1: Build file path -> title mapping
	console.log("Building file mappings...");
	const allFiles = (await $`find ${DOCS_ROOT} -name "*.md" -type f`.text()).trim().split("\n");
	const fileToTitle = new Map<string, string>();

	for (const file of allFiles) {
		const content = await readFile(file, "utf-8");
		const title = extractTitle(content);
		if (title) {
			const relPath = relative(DOCS_ROOT, file);
			fileToTitle.set(relPath, title);
		}
	}
	console.log(`  ${fileToTitle.size} files mapped to titles`);

	// Step 2: Build title -> nodeId mapping via semantic search
	console.log("Fetching nodes from NodeSpace...");
	const searchResult = await callMCP("tools/call", {
		name: "search_semantic",
		arguments: { query: "documentation guide architecture", limit: 200, threshold: 0, include_markdown: 0 }
	}) as { content: Array<{ text: string }> };

	const nodes = JSON.parse(searchResult.content[0].text).nodes as Array<{ id: string; title: string }>;
	const titleToNodeId = new Map<string, string>();
	for (const node of nodes) {
		titleToNodeId.set(node.title, node.id);
	}
	console.log(`  ${titleToNodeId.size} nodes found`);

	// Step 3: Build file path -> nodeId mapping
	const pathToNodeId = new Map<string, string>();
	for (const [path, title] of fileToTitle) {
		const nodeId = titleToNodeId.get(title);
		if (nodeId) {
			pathToNodeId.set(path, nodeId);
		}
	}
	console.log(`  ${pathToNodeId.size} files matched to nodes\n`);

	// Step 4: Find files with links
	const grepResult = await $`grep -rl "\\]([^)]*\\.md)" ${DOCS_ROOT} --include="*.md"`.text();
	const filesWithLinks = grepResult.trim().split("\n").filter(Boolean);
	console.log(`Found ${filesWithLinks.length} files with internal links\n`);

	// Step 5: Process each file
	let totalResolved = 0;
	let totalUnresolved = 0;
	let nodesUpdated = 0;

	for (const file of filesWithLinks) {
		const relPath = relative(DOCS_ROOT, file);
		const sourceNodeId = pathToNodeId.get(relPath);

		if (!sourceNodeId) {
			console.log(`  Skip: ${relPath} (no node found)`);
			continue;
		}

		// Get full markdown with node IDs
		const mdResult = await callMCP("tools/call", {
			name: "get_markdown_from_node_id",
			arguments: { node_id: sourceNodeId, include_node_ids: true }
		}) as { content: Array<{ text: string }> };

		const mdData = JSON.parse(mdResult.content[0].text);
		const markdown = mdData.markdown as string;

		// Find all child node IDs and their content
		const nodePattern = /<!-- ([a-f0-9-]+) v\d+ -->\n([^\n]+)/g;
		let nodeMatch;
		const nodesToUpdate: Array<{ id: string; oldContent: string; newContent: string }> = [];

		while ((nodeMatch = nodePattern.exec(markdown)) !== null) {
			const [, nodeId, content] = nodeMatch;
			const links = extractLinks(content);

			if (links.length === 0) continue;

			let newContent = content;
			let resolved = 0;

			for (const { full, text, path } of links) {
				const targetPath = resolveToRelative(relPath, path);
				const targetNodeId = pathToNodeId.get(targetPath);

				if (targetNodeId) {
					const newLink = `[${text}](nodespace://${targetNodeId})`;
					newContent = newContent.replace(full, newLink);
					resolved++;
					totalResolved++;
				} else {
					totalUnresolved++;
				}
			}

			if (resolved > 0 && newContent !== content) {
				nodesToUpdate.push({ id: nodeId, oldContent: content, newContent });
			}
		}

		if (nodesToUpdate.length > 0) {
			console.log(`  ${relPath}: ${nodesToUpdate.length} nodes to update`);

			if (!dryRun) {
				for (const { id, newContent } of nodesToUpdate) {
					await callMCP("tools/call", {
						name: "update_node",
						arguments: { node_id: id, content: newContent }
					});
					nodesUpdated++;
				}
			}
		}
	}

	console.log(`\n=== Summary ===`);
	console.log(`Links resolved: ${totalResolved}`);
	console.log(`Links unresolved: ${totalUnresolved}`);
	console.log(`Nodes updated: ${nodesUpdated}`);

	if (dryRun) {
		console.log("\nRun without --dry-run to apply changes.");
	}
}

main().catch(console.error);
