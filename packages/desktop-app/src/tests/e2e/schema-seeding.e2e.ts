/**
 * E2E: Schema seeding verification
 *
 * Verifies that core schemas are populated after daemon startup, covering
 * the startup timing requirement.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { DaemonTestHarness } from './daemon-harness';

let h: DaemonTestHarness;

beforeAll(async () => {
  h = await DaemonTestHarness.start();
}, 15_000);

afterAll(async () => {
  await h?.stop();
});

describe('Schema seeding after daemon startup', () => {
  it('getAllSchemas returns core schemas immediately after start', async () => {
    const schemas = await h.adapter.getAllSchemas();
    expect(schemas.length).toBeGreaterThan(0);
  });

  it('core schemas have the isCore flag set to true', async () => {
    const schemas = await h.adapter.getAllSchemas();
    const coreSchemas = schemas.filter((s) => s.isCore === true);
    expect(coreSchemas.length).toBeGreaterThan(0);
  });

  it('each schema has an id and nodeType', async () => {
    const schemas = await h.adapter.getAllSchemas();
    for (const schema of schemas) {
      expect(typeof schema.id).toBe('string');
      expect((schema.id as string).length).toBeGreaterThan(0);
      expect(typeof schema.nodeType).toBe('string');
    }
  });

  it('getSchema returns a specific core schema by id', async () => {
    const schemas = await h.adapter.getAllSchemas();
    expect(schemas.length).toBeGreaterThan(0);

    const first = schemas[0];
    const fetched = await h.adapter.getSchema(first.id as string);

    expect(fetched.id).toBe(first.id);
    expect(fetched.isCore).toBe(first.isCore);
  });

  it('schemas carry their relationships and the type they extend', async () => {
    const schemas = await h.adapter.getAllSchemas();
    const byId = (id: string) => schemas.find((s) => s.id === id);

    // A core subtype names its parent in `extends`, never as a relationship.
    const native = byId('ai-chat-native');
    expect(native?.extends).toBe('ai-chat');
    expect(native?.relationships.some((r) => r.name === 'extends')).toBe(false);

    // A type with declared relationships lists them.
    const task = byId('task');
    expect(task?.extends).toBeUndefined();
    expect(task?.relationships.map((r) => r.name)).toContain('blocks');

    // Typed top-level fields: nothing is smuggled through `properties`.
    for (const schema of schemas) {
      expect(Array.isArray(schema.relationships)).toBe(true);
      expect(schema.properties).toEqual({});
      expect(schema).not.toHaveProperty('description');
    }
  });

  it('getSchema returns the relationships and the type a schema extends', async () => {
    const native = await h.adapter.getSchema('ai-chat-native');
    expect(native.extends).toBe('ai-chat');

    const task = await h.adapter.getSchema('task');
    expect(task.relationships.map((r) => r.name)).toContain('blocks');
  });

  it('schemas have a fields array', async () => {
    const schemas = await h.adapter.getAllSchemas();
    for (const schema of schemas) {
      expect(Array.isArray(schema.fields)).toBe(true);
    }
  });
});
