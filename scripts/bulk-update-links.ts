#!/usr/bin/env bun
/**
 * Bulk update internal markdown links to nodespace:// URIs
 * Runs directly against SurrealDB for speed
 */

import { readFile } from "fs/promises";
import { join, relative, dirname, normalize } from "path";
import { $ } from "bun";

const DOCS_ROOT = join(import.meta.dir, "..", "docs");
const SURREAL_ENDPOINT = "http://127.0.0.1:8000/sql";

async function querySurreal(sql: string): Promise<unknown> {
	const response = await fetch(SURREAL_ENDPOINT, {
		method: "POST",
		headers: {
			"Accept": "application/json",
			"surreal-ns": "nodespace",
			"surreal-db": "nodespace",
			"Authorization": "Basic " + btoa("root:root")
		},
		body: sql
	});
	const result = await response.json() as Array<{ result: unknown; status: string; time: string }>;
	if (result[0]?.status === "ERR") {
		throw new Error(String(result[0].result));
	}
	return result[0]?.result;
}

function extractTitle(content: string): string | null {
	const match = content.match(/^#\s+(.+)$/m);
	return match ? match[1].trim() : null;
}

function resolveToRelative(fromFile: string, linkPath: string): string {
	const fromDir = dirname(fromFile);
	const joined = join(fromDir, linkPath);
	return normalize(joined).replace(/\\/g, "/");
}

function extractNodeId(surrealId: string): string | null {
	const match = surrealId.match(/node:⟨([^⟩]+)⟩/);
	return match ? match[1] : null;
}

// Cache for root lookups
const rootCache = new Map<string, string | null>();

async function findRootNodeId(nodeId: string): Promise<string | null> {
	if (rootCache.has(nodeId)) {
		return rootCache.get(nodeId)!;
	}

	// Check if this node has a title (is a root)
	const nodeResult = await querySurreal(
		`SELECT title FROM node:⟨${nodeId}⟩;`
	) as Array<{ title: string | null }>;

	if (nodeResult.length > 0 && nodeResult[0].title) {
		rootCache.set(nodeId, nodeId);
		return nodeId;
	}

	// Find parent via has_child relationship (in = parent, out = child)
	const parentResult = await querySurreal(
		`SELECT in FROM relationship WHERE out = node:⟨${nodeId}⟩ AND relationship_type = "has_child" LIMIT 1;`
	) as Array<{ in: string }>;

	if (parentResult.length === 0) {
		rootCache.set(nodeId, null);
		return null;
	}

	const parentId = extractNodeId(parentResult[0].in);
	if (!parentId) {
		rootCache.set(nodeId, null);
		return null;
	}

	// Recurse to find root
	const rootId = await findRootNodeId(parentId);
	rootCache.set(nodeId, rootId);
	return rootId;
}

async function main() {
	const dryRun = process.argv.includes("--dry-run");
	console.log(`=== Bulk Link Update ${dryRun ? "[DRY-RUN]" : ""} ===\n`);

	// Step 1: Build file path -> title mapping from local docs
	console.log("Building file mappings...");
	const allFiles = (await $`find ${DOCS_ROOT} -name "*.md" -type f`.text()).trim().split("\n");
	const fileToTitle = new Map<string, string>();
	const titleToFile = new Map<string, string>();

	for (const file of allFiles) {
		const content = await readFile(file, "utf-8");
		const title = extractTitle(content);
		if (title) {
			const relPath = relative(DOCS_ROOT, file);
			fileToTitle.set(relPath, title);
			titleToFile.set(title, relPath);
		}
	}
	console.log(`  ${fileToTitle.size} files mapped to titles`);

	// Step 2: Get all root nodes from database
	console.log("Fetching root nodes from database...");
	const rootNodes = await querySurreal(
		`SELECT id, title FROM node WHERE title IS NOT NONE;`
	) as Array<{ id: string; title: string }>;

	console.log(`  ${rootNodes.length} root nodes found`);

	// Build mappings
	const titleToNodeId = new Map<string, string>();
	const nodeIdToTitle = new Map<string, string>();

	for (const node of rootNodes) {
		const nodeId = extractNodeId(node.id);
		if (nodeId) {
			titleToNodeId.set(node.title, nodeId);
			nodeIdToTitle.set(nodeId, node.title);
		}
	}

	// Build file path -> nodeId mapping for link targets
	const pathToNodeId = new Map<string, string>();
	for (const [path, title] of fileToTitle) {
		const nodeId = titleToNodeId.get(title);
		if (nodeId) {
			pathToNodeId.set(path, nodeId);
		}
	}
	console.log(`  ${pathToNodeId.size} files matched to nodes\n`);

	// Step 3: Get all nodes with .md) links
	console.log("Finding nodes with internal links...");
	const nodesWithLinks = await querySurreal(
		`SELECT id, content FROM node WHERE content CONTAINS ".md)";`
	) as Array<{ id: string; content: string }>;

	console.log(`  ${nodesWithLinks.length} nodes with potential links\n`);

	// Step 4: Process each node
	let totalUpdated = 0;
	let totalLinksResolved = 0;
	let totalLinksUnresolved = 0;
	let skipped = 0;

	for (let i = 0; i < nodesWithLinks.length; i++) {
		const node = nodesWithLinks[i];
		if (node.content.includes("nodespace://")) continue; // Already has converted links
		if (!node.content.match(/\[[^\]]+\]\([^)]+\.md\)/)) continue; // No actual md links

		const nodeId = extractNodeId(node.id);
		if (!nodeId) continue;

		// Find the root node to determine source file
		const rootId = await findRootNodeId(nodeId);
		if (!rootId) {
			skipped++;
			continue;
		}

		const rootTitle = nodeIdToTitle.get(rootId);
		if (!rootTitle) {
			skipped++;
			continue;
		}

		const sourceFile = titleToFile.get(rootTitle);
		if (!sourceFile) {
			skipped++;
			continue;
		}

		// Find and replace all .md links
		const matches: Array<{ full: string; text: string; path: string }> = [];
		const linkPattern = /\[([^\]]+)\]\(([^)]+\.md)\)/g;
		let match;
		while ((match = linkPattern.exec(node.content)) !== null) {
			if (!match[2].startsWith("http") && !match[2].startsWith("nodespace://")) {
				matches.push({ full: match[0], text: match[1], path: match[2] });
			}
		}

		if (matches.length === 0) continue;

		let newContent = node.content;
		let hasChanges = false;

		for (const { full, text, path } of matches) {
			const targetPath = resolveToRelative(sourceFile, path);
			const targetNodeId = pathToNodeId.get(targetPath);

			if (targetNodeId) {
				const newLink = `[${text}](nodespace://${targetNodeId})`;
				newContent = newContent.replace(full, newLink);
				hasChanges = true;
				totalLinksResolved++;
			} else {
				totalLinksUnresolved++;
			}
		}

		if (hasChanges) {
			if (dryRun) {
				console.log(`  [DRY] ${i + 1}/${nodesWithLinks.length} Would update in ${sourceFile}`);
			} else {
				// Escape for SQL
				const escapedContent = newContent
					.replace(/\\/g, "\\\\")
					.replace(/'/g, "\\'");

				await querySurreal(
					`UPDATE node:⟨${nodeId}⟩ SET content = '${escapedContent}', version = version + 1, modified_at = time::now();`
				);
				if ((totalUpdated + 1) % 10 === 0) {
					console.log(`  ${totalUpdated + 1} nodes updated...`);
				}
			}
			totalUpdated++;
		}
	}

	console.log(`\n=== Summary ===`);
	console.log(`Nodes updated: ${totalUpdated}`);
	console.log(`Nodes skipped: ${skipped}`);
	console.log(`Links resolved: ${totalLinksResolved}`);
	console.log(`Links unresolved: ${totalLinksUnresolved}`);

	if (dryRun) {
		console.log("\nRun without --dry-run to apply changes.");
	}
}

main().catch(console.error);
