/**
 * Schema Initialization Script via Dev Proxy
 *
 * This script initializes core schema nodes by POSTing them through the dev-proxy,
 * which handles SurrealDB communication and business logic validation.
 *
 * Flow:
 * 1. Wait for dev-proxy health check (http://localhost:3001/health)
 * 2. Create 7 core schema nodes via POST /api/nodes
 * 3. Each schema is a special node with nodeType="schema"
 *
 * Run this script after dev-proxy is running (dev:proxy in concurrent dev mode).
 *
 * @example
 * ```bash
 * # In concurrent dev mode (automatic):
 * bun run dev
 *
 * # Manual initialization (if needed):
 * bun run dev:db:init
 * ```
 */

const PROXY_URL = 'http://localhost:3001';
const MAX_RETRIES = 30;
const RETRY_DELAY_MS = 1000;

interface CreateNodeRequest {
  id?: string;
  nodeType: string;
  content: string;
  parentId?: string | null;
  beforeSiblingId?: string | null;
  properties?: Record<string, unknown>;
}

interface CreateNodeResponse {
  id: string;
  nodeType: string;
  content: string;
  version: number;
  createdAt: string;
  modifiedAt: string;
  properties?: Record<string, unknown>;
}

/**
 * Wait for dev-proxy to be ready
 */
async function waitForProxy(): Promise<void> {
  console.log('🔍 Waiting for dev-proxy to be ready...');

  for (let i = 0; i < MAX_RETRIES; i++) {
    try {
      const response = await globalThis.fetch(`${PROXY_URL}/health`, {
        method: 'GET'
      });

      if (response.ok) {
        console.log('✅ Dev-proxy is ready\n');
        return;
      }
    } catch {
      // Connection failed, retry
    }

    if (i < MAX_RETRIES - 1) {
      await new Promise((resolve) => setTimeout(resolve, RETRY_DELAY_MS));
    }
  }

  throw new Error(
    `Dev-proxy not responding after ${MAX_RETRIES} attempts. Ensure dev-proxy is running.`
  );
}

/**
 * Create a schema node via dev-proxy
 */
async function createSchemaNode(request: CreateNodeRequest): Promise<CreateNodeResponse> {
  // Add required timestamp fields
  const now = new Date().toISOString();
  const fullRequest = {
    ...request,
    createdAt: now,
    modifiedAt: now,
    version: 1,
    mentions: [],
    mentionedBy: []
  };

  const response = await globalThis.fetch(`${PROXY_URL}/api/nodes`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json'
    },
    body: JSON.stringify(fullRequest)
  });

  if (!response.ok) {
    const error = await response.text();
    throw new Error(`HTTP ${response.status}: ${error}`);
  }

  return response.json();
}

/**
 * Check if schemas already exist by trying to get the task schema
 */
async function checkSchemasExist(): Promise<boolean> {
  try {
    const response = await globalThis.fetch(`${PROXY_URL}/api/nodes/task`, {
      method: 'GET'
    });

    // If we get the task schema back, schemas are already seeded
    if (response.ok) {
      const node = await response.json();
      return node.nodeType === 'schema';
    }
    return false;
  } catch {
    return false;
  }
}

/**
 * Seed all core schemas
 */
async function seedCoreSchemas(): Promise<void> {
  console.log('🌱 Seeding core schemas...\n');

  // Check if already seeded
  const exists = await checkSchemasExist();
  if (exists) {
    console.log('ℹ️  Core schemas already seeded, skipping...\n');
    return;
  }

  // Task schema
  console.log('  📝 Creating task schema...');
  await createSchemaNode({
    id: 'task',
    nodeType: 'schema',
    content: 'Task',
    properties: {
      is_core: true,
      version: 1,
      description: 'Task tracking schema',
      fields: [
        {
          name: 'status',
          type: 'enum',
          protection: 'core',
          core_values: ['OPEN', 'IN_PROGRESS', 'DONE'],
          user_values: [],
          indexed: true,
          required: true,
          extensible: true,
          default: 'OPEN',
          description: 'Task status'
        },
        {
          name: 'priority',
          type: 'enum',
          protection: 'user',
          core_values: ['LOW', 'MEDIUM', 'HIGH'],
          user_values: [],
          indexed: true,
          required: false,
          extensible: true,
          description: 'Task priority'
        },
        {
          name: 'due_date',
          type: 'date',
          protection: 'user',
          indexed: true,
          required: false,
          description: 'Due date'
        },
        {
          name: 'started_at',
          type: 'date',
          protection: 'user',
          indexed: false,
          required: false,
          description: 'Started at'
        },
        {
          name: 'completed_at',
          type: 'date',
          protection: 'user',
          indexed: false,
          required: false,
          description: 'Completed at'
        },
        {
          name: 'assignee',
          type: 'text',
          protection: 'user',
          indexed: true,
          required: false,
          description: 'Assignee'
        }
      ]
    }
  });

  // Date schema
  console.log('  📅 Creating date schema...');
  await createSchemaNode({
    id: 'date',
    nodeType: 'schema',
    content: 'Date',
    properties: {
      is_core: true,
      version: 1,
      description: 'Date node schema',
      fields: []
    }
  });

  // Text schema
  console.log('  📄 Creating text schema...');
  await createSchemaNode({
    id: 'text',
    nodeType: 'schema',
    content: 'Text',
    properties: {
      is_core: true,
      version: 1,
      description: 'Plain text content',
      fields: []
    }
  });

  // Header schema
  console.log('  🔤 Creating header schema...');
  await createSchemaNode({
    id: 'header',
    nodeType: 'schema',
    content: 'Header',
    properties: {
      is_core: true,
      version: 1,
      description: 'Markdown header (h1-h6)',
      fields: []
    }
  });

  // Code block schema
  console.log('  💻 Creating code-block schema...');
  await createSchemaNode({
    id: 'code-block',
    nodeType: 'schema',
    content: 'Code Block',
    properties: {
      is_core: true,
      version: 1,
      description: 'Code block with syntax highlighting',
      fields: []
    }
  });

  // Quote block schema
  console.log('  💬 Creating quote-block schema...');
  await createSchemaNode({
    id: 'quote-block',
    nodeType: 'schema',
    content: 'Quote Block',
    properties: {
      is_core: true,
      version: 1,
      description: 'Blockquote for citations',
      fields: []
    }
  });

  // Ordered list schema
  console.log('  🔢 Creating ordered-list schema...');
  await createSchemaNode({
    id: 'ordered-list',
    nodeType: 'schema',
    content: 'Ordered List',
    properties: {
      is_core: true,
      version: 1,
      description: 'Numbered list item',
      fields: []
    }
  });

  console.log('\n✅ All core schemas seeded successfully\n');
}

/**
 * Main initialization function
 */
async function initialize(): Promise<void> {
  console.log('🚀 Initializing schemas via dev-proxy...\n');

  try {
    await waitForProxy();
    await seedCoreSchemas();

    console.log('✨ Schema initialization complete!\n');
    console.log('📊 You can now inspect the schemas:');
    console.log(`   Dev-proxy: ${PROXY_URL}`);
    console.log(`   Schemas: GET ${PROXY_URL}/api/nodes/<schema-id>`);
    console.log(`   Available schemas: task, date, text, header, code-block, quote-block, ordered-list\n`);
  } catch (error) {
    console.error('\n❌ Initialization failed:', error);
    process.exit(1);
  }
}

// Run initialization
initialize();
