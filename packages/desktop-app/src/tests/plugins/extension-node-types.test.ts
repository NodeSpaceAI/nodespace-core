/**
 * Node-type contributions (ADR-082 §2.1, §3.2): registering an extension
 * registers its `PluginDefinition`s with the plugin registry, so the viewer's
 * node-component loader resolves the type's lazily loaded component like any
 * plugin type's, and unregistering the extension takes the type away again.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import type { Component } from 'svelte';
import { NodeComponentLoader } from '$lib/design/components/node-component-loader.svelte';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { uiExtensionRegistry } from '$lib/plugins/ui-extensions';
import type { NodeComponentProps } from '$lib/types/node-viewers';
import {
  TEST_EXTENSION_ID,
  TEST_NODE_TYPE,
  createTestExtension,
  createTestNodePlugin,
  resetTestExtension,
  testExtensionMounts
} from '../fixtures/test-extension';

describe('node-type contributions', () => {
  afterEach(() => {
    cleanup();
    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    resetTestExtension();
  });

  it("resolves the extension's lazy node component for its type, which renders with the node's props", async () => {
    uiExtensionRegistry.register(createTestExtension());
    const loader = new NodeComponentLoader();

    await loader.load(TEST_NODE_TYPE);

    const NodeView = loader.get(TEST_NODE_TYPE) as Component<NodeComponentProps> | undefined;
    expect(NodeView).toBeTypeOf('function');
    const { findByTestId } = render(NodeView as Component<NodeComponentProps>, {
      props: { nodeId: 'node-7', content: 'Quarterly plan', nodeType: TEST_NODE_TYPE }
    });
    const el = await findByTestId('test-node');
    expect(el.getAttribute('data-node-id')).toBe('node-7');
    expect(el.textContent).toBe('Quarterly plan');
    expect(testExtensionMounts.node).toBe(1);
  });

  it('resolves nothing for the type once the extension is unregistered', async () => {
    uiExtensionRegistry.register(createTestExtension());
    await new NodeComponentLoader().load(TEST_NODE_TYPE);

    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    const loader = new NodeComponentLoader();
    await loader.load(TEST_NODE_TYPE);

    expect(loader.has(TEST_NODE_TYPE)).toBe(false);
    expect(pluginRegistry.hasPlugin(TEST_NODE_TYPE)).toBe(false);
  });

  it("cannot replace a core type's plugin, and unregistering leaves it in place", () => {
    const collectionPlugin = pluginRegistry.getPlugin('collection');
    expect(collectionPlugin).not.toBeNull();

    uiExtensionRegistry.register(
      createTestExtension({
        nodeTypes: [{ plugin: { ...createTestNodePlugin(), id: 'collection' } }]
      })
    );
    expect(pluginRegistry.getPlugin('collection')).toBe(collectionPlugin);

    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    expect(pluginRegistry.getPlugin('collection')).toBe(collectionPlugin);
  });
});
