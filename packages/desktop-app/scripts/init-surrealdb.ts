/**
 * SurrealDB Initialization Script for Browser Dev Mode
 *
 * This script initializes the SurrealDB database with the NodeSpace schema:
 * 1. Creates namespace and database
 * 2. Defines nodes table (SCHEMALESS for flexibility)
 * 3. Defines mentions table (graph relation table for node mentions)
 * 4. Seeds core schema definitions (task, text, date, etc.)
 *
 * Run this script after starting the SurrealDB server (`bun run dev:db`)
 *
 * @example
 * ```bash
 * # Start SurrealDB server
 * bun run dev:db
 *
 * # In another terminal, initialize schema
 * bun run dev:db:init
 * ```
 */

const SURREAL_URL = 'http://127.0.0.1:8000';
const AUTH_HEADER = 'Basic ' + globalThis.btoa('root:root');
const NAMESPACE = 'nodespace';
const DATABASE = 'nodes';

interface SurrealResponse {
  result?: unknown;
  status: 'OK' | 'ERR';
  time: string;
}

/**
 * Execute SurrealQL query
 * @param sql - SurrealQL query
 * @param useNamespace - Whether to prepend USE NS/DB statements (false for namespace creation)
 */
async function query(sql: string, useNamespace = true): Promise<SurrealResponse[]> {
  const headers: Record<string, string> = {
    Authorization: AUTH_HEADER,
    Accept: 'application/json'
  };

  // Prepend USE statements if namespace already exists
  // HTTP API doesn't maintain session state, so USE statements must be included in each request
  let fullSql = sql;
  if (useNamespace) {
    fullSql = `USE NS ${NAMESPACE}; USE DB ${DATABASE}; ${sql}`;
  }

  const response = await globalThis.fetch(`${SURREAL_URL}/sql`, {
    method: 'POST',
    headers,
    body: fullSql
  });

  if (!response.ok) {
    const error = await response.text();
    throw new Error(`HTTP ${response.status}: ${error}`);
  }

  const results = await response.json();

  // Check for SurrealDB errors
  // If useNamespace=true, results[0] and [1] are USE statements, actual query is results[2+]
  // If useNamespace=false, results start at [0]
  const startIndex = useNamespace ? 2 : 0;
  for (let i = startIndex; i < results.length; i++) {
    if (results[i].status === 'ERR') {
      throw new Error(`SurrealDB error: ${results[i].result}`);
    }
  }

  return results;
}

/**
 * Initialize namespace and database
 */
async function initializeNamespace(): Promise<void> {
  console.log(`📦 Creating namespace '${NAMESPACE}' and database '${DATABASE}'...`);

  // Create namespace and database in one query (use NS/DB in subsequent queries)
  // Note: In HTTP mode, each request is stateless, so we send NS/DB headers
  await query(
    `
    DEFINE NAMESPACE IF NOT EXISTS ${NAMESPACE};
    USE NS ${NAMESPACE};
    DEFINE DATABASE IF NOT EXISTS ${DATABASE};
  `,
    false
  );

  console.log('✅ Namespace and database created');
}

/**
 * Create nodes table (universal table for all node types)
 */
async function createNodesTable(): Promise<void> {
  console.log('🗄️  Creating nodes table...');

  await query(`
    DEFINE TABLE IF NOT EXISTS nodes SCHEMALESS;
  `);

  console.log('✅ Nodes table created');
}

/**
 * Create mentions table (graph relation table for node mentions)
 */
async function createMentionsTable(): Promise<void> {
  console.log('🗄️  Creating mentions graph relation table...');

  await query(`
    DEFINE TABLE IF NOT EXISTS mentions SCHEMALESS;
  `);

  console.log('✅ Mentions table created');
}

/**
 * Seed core schema definitions as nodes
 *
 * Core schemas are stored as nodes with node_type = "schema"
 * and schema definitions in the properties field
 */
async function seedCoreSchemas(): Promise<void> {
  console.log('🌱 Seeding core schemas...');

  // Check if schemas already exist
  // Result is at index 2 (after USE NS and USE DB statements)
  const results = await query(`SELECT * FROM nodes:\`task\` LIMIT 1;`);
  const queryResult = results[2].result;
  if (queryResult && Array.isArray(queryResult) && queryResult.length > 0) {
    console.log('ℹ️  Core schemas already seeded, skipping...');
    return;
  }

  // Task schema
  await query(`
    CREATE nodes:\`task\` CONTENT {
      uuid: "task",
      node_type: "schema",
      content: "Task",
      parent_id: NONE,
      container_node_id: NONE,
      before_sibling_id: NONE,
      version: 1,
      created_at: time::now(),
      modified_at: time::now(),
      properties: {
        is_core: true,
        version: 1,
        description: "Task tracking schema",
        fields: [
          {
            name: "status",
            field_type: "enum",
            protection: "core",
            core_values: ["OPEN", "IN_PROGRESS", "DONE"],
            user_values: [],
            indexed: true,
            required: true,
            extensible: true,
            default: "OPEN",
            description: "Task status"
          },
          {
            name: "priority",
            field_type: "enum",
            protection: "core",
            core_values: ["LOW", "MEDIUM", "HIGH"],
            user_values: [],
            indexed: false,
            required: false,
            extensible: true,
            default: "MEDIUM",
            description: "Task priority"
          }
        ]
      },
      mentions: [],
      mentioned_by: []
    };
  `);

  // Text schema
  await query(`
    CREATE nodes:\`text\` CONTENT {
      uuid: "text",
      node_type: "schema",
      content: "Text",
      parent_id: NONE,
      container_node_id: NONE,
      before_sibling_id: NONE,
      version: 1,
      created_at: time::now(),
      modified_at: time::now(),
      properties: {
        is_core: true,
        version: 1,
        description: "Plain text node schema",
        fields: []
      },
      mentions: [],
      mentioned_by: []
    };
  `);

  // Date schema
  await query(`
    CREATE nodes:\`date\` CONTENT {
      uuid: "date",
      node_type: "schema",
      content: "Date",
      parent_id: NONE,
      container_node_id: NONE,
      before_sibling_id: NONE,
      version: 1,
      created_at: time::now(),
      modified_at: time::now(),
      properties: {
        is_core: true,
        version: 1,
        description: "Date container schema",
        fields: [
          {
            name: "date",
            field_type: "date",
            protection: "core",
            indexed: true,
            required: true,
            default: null,
            description: "The date value"
          }
        ]
      },
      mentions: [],
      mentioned_by: []
    };
  `);

  // Header schema
  await query(`
    CREATE nodes:\`header\` CONTENT {
      uuid: "header",
      node_type: "schema",
      content: "Header",
      parent_id: NONE,
      container_node_id: NONE,
      before_sibling_id: NONE,
      version: 1,
      created_at: time::now(),
      modified_at: time::now(),
      properties: {
        is_core: true,
        version: 1,
        description: "Header/section schema",
        fields: [
          {
            name: "level",
            field_type: "number",
            protection: "core",
            indexed: false,
            required: true,
            default: 1,
            description: "Header level (1-6)"
          }
        ]
      },
      mentions: [],
      mentioned_by: []
    };
  `);

  // Code block schema
  await query(`
    CREATE nodes:\`code-block\` CONTENT {
      uuid: "code-block",
      node_type: "schema",
      content: "Code Block",
      parent_id: NONE,
      container_node_id: NONE,
      before_sibling_id: NONE,
      version: 1,
      created_at: time::now(),
      modified_at: time::now(),
      properties: {
        is_core: true,
        version: 1,
        description: "Code block schema",
        fields: [
          {
            name: "language",
            field_type: "string",
            protection: "core",
            indexed: false,
            required: false,
            default: "",
            description: "Programming language"
          }
        ]
      },
      mentions: [],
      mentioned_by: []
    };
  `);

  // Quote block schema
  await query(`
    CREATE nodes:\`quote-block\` CONTENT {
      uuid: "quote-block",
      node_type: "schema",
      content: "Quote Block",
      parent_id: NONE,
      container_node_id: NONE,
      before_sibling_id: NONE,
      version: 1,
      created_at: time::now(),
      modified_at: time::now(),
      properties: {
        is_core: true,
        version: 1,
        description: "Quote/blockquote schema",
        fields: []
      },
      mentions: [],
      mentioned_by: []
    };
  `);

  // Ordered list schema
  await query(`
    CREATE nodes:\`ordered-list\` CONTENT {
      uuid: "ordered-list",
      node_type: "schema",
      content: "Ordered List",
      parent_id: NONE,
      container_node_id: NONE,
      before_sibling_id: NONE,
      version: 1,
      created_at: time::now(),
      modified_at: time::now(),
      properties: {
        is_core: true,
        version: 1,
        description: "Ordered/numbered list schema",
        fields: []
      },
      mentions: [],
      mentioned_by: []
    };
  `);

  console.log('✅ Core schemas seeded successfully');
}

/**
 * Main initialization function
 */
async function initialize(): Promise<void> {
  console.log('🚀 Initializing SurrealDB for NodeSpace...\n');

  try {
    // Test connection
    console.log('🔍 Testing connection to SurrealDB...');
    const response = await globalThis.fetch(`${SURREAL_URL}/health`);
    if (!response.ok) {
      throw new Error('SurrealDB server is not responding. Run `bun run dev:db` first.');
    }
    console.log('✅ Connected to SurrealDB\n');

    // Initialize database
    await initializeNamespace();
    await createNodesTable();
    await createMentionsTable();
    await seedCoreSchemas();

    console.log('\n✨ SurrealDB initialization complete!');
    console.log(`\n📊 You can now inspect the database with Surrealist:`);
    console.log(`   URL: ${SURREAL_URL}`);
    console.log(`   Namespace: ${NAMESPACE}`);
    console.log(`   Database: ${DATABASE}`);
    console.log(`   Username: root`);
    console.log(`   Password: root\n`);
  } catch (error) {
    console.error('\n❌ Initialization failed:', error);
    process.exit(1);
  }
}

// Run initialization
initialize();
