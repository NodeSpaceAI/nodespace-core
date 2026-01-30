#!/usr/bin/env bun
/**
 * Create mentions relationships for all nodespace:// links in node content
 * Runs directly against SurrealDB
 *
 * Mentions relationship structure:
 * - in: source node (the node containing the link)
 * - out: target node (the node being linked to - should be a ROOT node)
 * - properties.root_id: the root node of the source (for backlinks panel)
 */

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

function extractNodeId(surrealId: string): string | null {
	const match = surrealId.match(/node:⟨([^⟩]+)⟩/);
	return match ? match[1] : null;
}

function isValidUUID(str: string): boolean {
	return /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i.test(str);
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
	const deleteOnly = process.argv.includes("--delete-only");
	console.log(`=== Create Mentions Relationships ${dryRun ? "[DRY-RUN]" : ""} ===\n`);

	if (deleteOnly) {
		console.log("Deleting all existing mentions relationships...");
		if (!dryRun) {
			await querySurreal(`DELETE relationship WHERE relationship_type = "mentions";`);
		}
		console.log("Done.");
		return;
	}

	// Get all nodes with nodespace:// links
	console.log("Finding nodes with nodespace:// links...");
	const nodesWithLinks = await querySurreal(
		`SELECT id, content FROM node WHERE content CONTAINS "nodespace://";`
	) as Array<{ id: string; content: string }>;

	console.log(`  ${nodesWithLinks.length} nodes with nodespace:// links\n`);

	// Build set of all root node IDs (nodes with titles)
	console.log("Fetching root nodes...");
	const rootNodes = await querySurreal(
		`SELECT id FROM node WHERE title IS NOT NONE;`
	) as Array<{ id: string }>;
	const rootNodeIds = new Set(rootNodes.map(n => extractNodeId(n.id)).filter(Boolean));
	console.log(`  ${rootNodeIds.size} root nodes found\n`);

	// Extract all mentions - need source node, its root, and target (must be a root)
	console.log("Processing mentions...");
	const linkPattern = /nodespace:\/\/([a-f0-9-]+)/g;
	const mentions: Array<{ sourceId: string; sourceRootId: string; targetId: string }> = [];
	let skippedNoRoot = 0;
	let skippedInvalidTarget = 0;

	for (const node of nodesWithLinks) {
		const sourceId = extractNodeId(node.id);
		if (!sourceId) continue;

		// Find root of source node
		const sourceRootId = await findRootNodeId(sourceId);
		if (!sourceRootId) {
			skippedNoRoot++;
			continue;
		}

		let match;
		while ((match = linkPattern.exec(node.content)) !== null) {
			const targetId = match[1];

			// Target must be a valid UUID and a root node
			if (!isValidUUID(targetId)) continue;

			if (!rootNodeIds.has(targetId)) {
				skippedInvalidTarget++;
				continue;
			}

			// Skip self-references at root level
			if (sourceRootId === targetId) continue;

			mentions.push({ sourceId, sourceRootId, targetId });
		}
		// Reset regex state
		linkPattern.lastIndex = 0;
	}

	console.log(`  ${mentions.length} valid mentions found`);
	console.log(`  ${skippedNoRoot} skipped (source has no root)`);
	console.log(`  ${skippedInvalidTarget} skipped (target not a root node)\n`);

	// Check existing mentions to avoid duplicates
	console.log("Checking existing mentions...");
	const existingMentions = await querySurreal(
		`SELECT in, out FROM relationship WHERE relationship_type = "mentions";`
	) as Array<{ in: string; out: string }>;

	const existingSet = new Set(
		existingMentions.map(r => {
			const inId = extractNodeId(r.in);
			const outId = extractNodeId(r.out);
			return `${inId}->${outId}`;
		})
	);

	console.log(`  ${existingMentions.length} existing mentions\n`);

	// Filter to only new mentions
	const newMentions = mentions.filter(
		m => !existingSet.has(`${m.sourceId}->${m.targetId}`)
	);

	console.log(`Creating ${newMentions.length} new mentions...\n`);

	if (dryRun) {
		for (const { sourceId, sourceRootId, targetId } of newMentions.slice(0, 10)) {
			console.log(`  [DRY] ${sourceId} -> ${targetId} (root: ${sourceRootId})`);
		}
		if (newMentions.length > 10) {
			console.log(`  ... and ${newMentions.length - 10} more`);
		}
	} else {
		let created = 0;
		let skipped = 0;
		for (const { sourceId, sourceRootId, targetId } of newMentions) {
			try {
				// Create relationship with properties.root_id
				await querySurreal(
					`RELATE node:⟨${sourceId}⟩->relationship->node:⟨${targetId}⟩ CONTENT {
						relationship_type: "mentions",
						properties: { root_id: "${sourceRootId}" },
						created_at: time::now(),
						modified_at: time::now(),
						version: 1
					};`
				);
				created++;
				if (created % 20 === 0) {
					console.log(`  ${created} mentions created...`);
				}
			} catch (e) {
				// Skip duplicates or other errors
				skipped++;
			}
		}
		console.log(`  ${created} mentions created, ${skipped} skipped (duplicates/errors)`);
	}

	console.log(`\n=== Summary ===`);
	console.log(`Total valid mentions found: ${mentions.length}`);
	console.log(`Already existing: ${mentions.length - newMentions.length}`);
	console.log(`New mentions ${dryRun ? "would be " : ""}created: ${newMentions.length}`);

	if (dryRun) {
		console.log("\nRun without --dry-run to apply changes.");
	}
}

main().catch(console.error);
