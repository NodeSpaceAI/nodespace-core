/**
 * PluginRegistry cache consistency: disabled or unregistered plugins must not
 * hand out cached components, and a lazy load that settles after unregister
 * must not repopulate the cache.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { PluginRegistry } from '$lib/plugins/plugin-registry';
import type {
  PluginDefinition,
  NodeViewerComponent,
  NodeComponent,
  SchemaFormComponent
} from '$lib/plugins/types';

const Viewer = (() => {}) as unknown as NodeViewerComponent;
const NodeComp = (() => {}) as unknown as NodeComponent;
const Form = (() => {}) as unknown as SchemaFormComponent;
const OldViewer = (() => {}) as unknown as NodeViewerComponent;
const NewViewer = (() => {}) as unknown as NodeViewerComponent;

function basePlugin(extra: Partial<PluginDefinition>): PluginDefinition {
  return {
    id: 'p',
    name: 'P',
    description: 'test',
    version: '1.0.0',
    config: { slashCommands: [] },
    ...extra
  };
}

describe('PluginRegistry cache consistency', () => {
  let registry: PluginRegistry;

  beforeEach(() => {
    registry = new PluginRegistry();
  });

  it('returns null from getViewer after setEnabled(false) even when loaded', async () => {
    registry.register(basePlugin({ viewer: { component: Viewer, priority: 1 } }));
    expect(await registry.getViewer('p')).toBe(Viewer);
    registry.setEnabled('p', false);
    expect(await registry.getViewer('p')).toBeNull();
    registry.setEnabled('p', true);
    expect(await registry.getViewer('p')).toBe(Viewer);
  });

  it('returns null from getNodeComponent after setEnabled(false) even when loaded', async () => {
    registry.register(basePlugin({ node: { component: NodeComp, priority: 1 } }));
    expect(await registry.getNodeComponent('p')).toBe(NodeComp);
    registry.setEnabled('p', false);
    expect(await registry.getNodeComponent('p')).toBeNull();
  });

  it('returns null from getSchemaForm after setEnabled(false) even when loaded', async () => {
    registry.register(basePlugin({ schemaForm: { component: Form, priority: 1 } }));
    expect(await registry.getSchemaForm('p')).toBe(Form);
    registry.setEnabled('p', false);
    expect(await registry.getSchemaForm('p')).toBeNull();
  });

  it('does not cache a viewer whose lazy load settles after unregister', async () => {
    let resolveOld!: (m: { default: NodeViewerComponent }) => void;
    registry.register(
      basePlugin({
        viewer: {
          lazyLoad: () => new Promise((r) => (resolveOld = r)),
          priority: 1
        }
      })
    );
    const pending = registry.getViewer('p');
    registry.unregister('p');
    registry.register(basePlugin({ viewer: { component: NewViewer, priority: 1 } }));
    resolveOld({ default: OldViewer });
    expect(await pending).toBeNull();
    expect(await registry.getViewer('p')).toBe(NewViewer);
  });

  it('does not cache a node component whose lazy load settles after unregister', async () => {
    let resolveOld!: (m: { default: NodeComponent }) => void;
    registry.register(
      basePlugin({
        node: { lazyLoad: () => new Promise((r) => (resolveOld = r)), priority: 1 }
      })
    );
    const pending = registry.getNodeComponent('p');
    registry.unregister('p');
    registry.register(basePlugin({ node: { component: NodeComp, priority: 1 } }));
    resolveOld({ default: (() => {}) as unknown as NodeComponent });
    expect(await pending).toBeNull();
    expect(await registry.getNodeComponent('p')).toBe(NodeComp);
  });

  it('does not cache a schema form whose lazy load settles after unregister', async () => {
    let resolveOld!: (m: { default: SchemaFormComponent }) => void;
    registry.register(
      basePlugin({
        schemaForm: { lazyLoad: () => new Promise((r) => (resolveOld = r)), priority: 1 }
      })
    );
    const pending = registry.getSchemaForm('p');
    registry.unregister('p');
    registry.register(basePlugin({ schemaForm: { component: Form, priority: 1 } }));
    resolveOld({ default: (() => {}) as unknown as SchemaFormComponent });
    expect(await pending).toBeNull();
    expect(await registry.getSchemaForm('p')).toBe(Form);
  });
});
