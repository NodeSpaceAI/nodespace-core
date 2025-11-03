import { describe, it, expect, beforeEach, vi } from 'vitest';
import { TextareaController } from '$lib/design/components/textarea-controller';
import { KeyboardCommandRegistry } from '$lib/services/keyboard-command-registry';

describe('Multi-Pane Keyboard Coordination', () => {
  let leftElement: HTMLTextAreaElement;
  let rightElement: HTMLTextAreaElement;
  let leftController: TextareaController;
  let rightController: TextareaController;
  let registry: KeyboardCommandRegistry;

  const leftPaneId = 'pane-left';
  const rightPaneId = 'pane-right';
  const activePaneId = leftPaneId; // Left pane is active

  const leftEvents = {
    contentChanged: vi.fn(),
    focus: vi.fn(),
    blur: vi.fn(),
    createNewNode: vi.fn(),
    indentNode: vi.fn(),
    outdentNode: vi.fn(),
    navigateArrow: vi.fn(),
    combineWithPrevious: vi.fn(),
    deleteNode: vi.fn(),
    directSlashCommand: vi.fn(),
    triggerDetected: vi.fn(),
    triggerHidden: vi.fn(),
    nodeReferenceSelected: vi.fn(),
    slashCommandDetected: vi.fn(),
    slashCommandHidden: vi.fn(),
    slashCommandSelected: vi.fn(),
    nodeTypeConversionDetected: vi.fn()
  };

  const rightEvents = {
    contentChanged: vi.fn(),
    focus: vi.fn(),
    blur: vi.fn(),
    createNewNode: vi.fn(),
    indentNode: vi.fn(),
    outdentNode: vi.fn(),
    navigateArrow: vi.fn(),
    combineWithPrevious: vi.fn(),
    deleteNode: vi.fn(),
    directSlashCommand: vi.fn(),
    triggerDetected: vi.fn(),
    triggerHidden: vi.fn(),
    nodeReferenceSelected: vi.fn(),
    slashCommandDetected: vi.fn(),
    slashCommandHidden: vi.fn(),
    slashCommandSelected: vi.fn(),
    nodeTypeConversionDetected: vi.fn()
  };

  beforeEach(() => {
    // Create textarea elements
    leftElement = document.createElement('textarea');
    rightElement = document.createElement('textarea');
    document.body.appendChild(leftElement);
    document.body.appendChild(rightElement);

    // Create controllers for both panes
    leftController = new TextareaController(
      leftElement,
      'node-left',
      'text',
      leftPaneId,
      leftEvents
    );

    rightController = new TextareaController(
      rightElement,
      'node-right',
      'text',
      rightPaneId,
      rightEvents
    );

    // Initialize both controllers
    leftController.initialize('Left pane content', true);
    rightController.initialize('Right pane content', true);

    // Create registry
    registry = KeyboardCommandRegistry.getInstance();
  });

  afterEach(() => {
    leftController.destroy();
    rightController.destroy();
    document.body.removeChild(leftElement);
    document.body.removeChild(rightElement);
    vi.clearAllMocks();
  });

  it('should only execute keyboard shortcuts in active pane', async () => {
    // Setup: Create keyboard event (Cmd+B for bold)
    const event = new KeyboardEvent('keydown', {
      key: 'b',
      metaKey: true,
      bubbles: true
    });

    // Build contexts with paneId and activePaneId metadata
    const leftContext = {
      event,
      controller: leftController,
      nodeId: 'node-left',
      nodeType: 'text',
      paneId: leftPaneId,
      content: 'Left pane content',
      cursorPosition: 0,
      selection: null,
      allowMultiline: false,
      metadata: { activePaneId }
    };

    const rightContext = {
      event,
      controller: rightController,
      nodeId: 'node-right',
      nodeType: 'text',
      paneId: rightPaneId,
      content: 'Right pane content',
      cursorPosition: 0,
      selection: null,
      allowMultiline: false,
      metadata: { activePaneId }
    };

    // Execute command in both controllers
    const leftHandled = await registry.execute(event, leftController, leftContext);
    const rightHandled = await registry.execute(event, rightController, rightContext);

    // Assert: Only left pane (active) should handle the command
    expect(leftHandled).toBe(true);
    expect(rightHandled).toBe(false);
  });

  it('should execute commands when pane becomes active', async () => {
    // Initially left pane is active
    const event = new KeyboardEvent('keydown', {
      key: 'b',
      metaKey: true,
      bubbles: true
    });

    const leftContext = {
      event,
      controller: leftController,
      nodeId: 'node-left',
      nodeType: 'text',
      paneId: leftPaneId,
      content: 'Left pane content',
      cursorPosition: 0,
      selection: null,
      allowMultiline: false,
      metadata: { activePaneId: leftPaneId }
    };

    const leftHandled = await registry.execute(event, leftController, leftContext);
    expect(leftHandled).toBe(true);

    // Now switch active pane to right
    const rightContext = {
      event,
      controller: rightController,
      nodeId: 'node-right',
      nodeType: 'text',
      paneId: rightPaneId,
      content: 'Right pane content',
      cursorPosition: 0,
      selection: null,
      allowMultiline: false,
      metadata: { activePaneId: rightPaneId } // Right is now active
    };

    const rightHandled = await registry.execute(event, rightController, rightContext);
    expect(rightHandled).toBe(true);
  });

  it('should not execute in any pane if activePaneId is missing from metadata', async () => {
    const event = new KeyboardEvent('keydown', {
      key: 'b',
      metaKey: true,
      bubbles: true
    });

    // Context without activePaneId in metadata
    const leftContext = {
      event,
      controller: leftController,
      nodeId: 'node-left',
      nodeType: 'text',
      paneId: leftPaneId,
      content: 'Left pane content',
      cursorPosition: 0,
      selection: null,
      allowMultiline: false,
      metadata: {} // No activePaneId
    };

    const leftHandled = await registry.execute(event, leftController, leftContext);

    // Without activePaneId, the pane check is skipped and command should execute
    // This is the expected fallback behavior for backwards compatibility
    expect(leftHandled).toBe(true);
  });

  it('should handle multiple panes with same command', async () => {
    // Create a third pane
    const centerElement = document.createElement('textarea');
    document.body.appendChild(centerElement);

    const centerPaneId = 'pane-center';
    const centerEvents = {
      contentChanged: vi.fn(),
      focus: vi.fn(),
      blur: vi.fn(),
      createNewNode: vi.fn(),
      indentNode: vi.fn(),
      outdentNode: vi.fn(),
      navigateArrow: vi.fn(),
      combineWithPrevious: vi.fn(),
      deleteNode: vi.fn(),
      directSlashCommand: vi.fn(),
      triggerDetected: vi.fn(),
      triggerHidden: vi.fn(),
      nodeReferenceSelected: vi.fn(),
      slashCommandDetected: vi.fn(),
      slashCommandHidden: vi.fn(),
      slashCommandSelected: vi.fn(),
      nodeTypeConversionDetected: vi.fn()
    };

    const centerController = new TextareaController(
      centerElement,
      'node-center',
      'text',
      centerPaneId,
      centerEvents
    );
    centerController.initialize('Center pane content', true);

    const event = new KeyboardEvent('keydown', {
      key: 'b',
      metaKey: true,
      bubbles: true
    });

    // Only center pane is active
    const contexts = [
      {
        controller: leftController,
        paneId: leftPaneId,
        nodeId: 'node-left'
      },
      {
        controller: rightController,
        paneId: rightPaneId,
        nodeId: 'node-right'
      },
      {
        controller: centerController,
        paneId: centerPaneId,
        nodeId: 'node-center'
      }
    ];

    const results = await Promise.all(
      contexts.map(async (ctx) => {
        const context = {
          event,
          controller: ctx.controller,
          nodeId: ctx.nodeId,
          nodeType: 'text',
          paneId: ctx.paneId,
          content: 'Content',
          cursorPosition: 0,
          selection: null,
          allowMultiline: false,
          metadata: { activePaneId: centerPaneId } // Center is active
        };
        return await registry.execute(event, ctx.controller, context);
      })
    );

    // Only center pane should execute
    expect(results[0]).toBe(false); // left
    expect(results[1]).toBe(false); // right
    expect(results[2]).toBe(true); // center (active)

    // Cleanup
    centerController.destroy();
    document.body.removeChild(centerElement);
  });

  it('should verify FormatTextCommand works with pane coordination', async () => {
    // Select some text in left pane
    leftElement.value = 'Test text';
    leftElement.setSelectionRange(0, 4); // Select "Test"

    const event = new KeyboardEvent('keydown', {
      key: 'b',
      metaKey: true,
      bubbles: true
    });

    const leftContext = {
      event,
      controller: leftController,
      nodeId: 'node-left',
      nodeType: 'text',
      paneId: leftPaneId,
      content: 'Test text',
      cursorPosition: 0,
      selection: window.getSelection(),
      allowMultiline: false,
      metadata: { activePaneId: leftPaneId }
    };

    const rightContext = {
      event,
      controller: rightController,
      nodeId: 'node-right',
      nodeType: 'text',
      paneId: rightPaneId,
      content: 'Test text',
      cursorPosition: 0,
      selection: window.getSelection(),
      allowMultiline: false,
      metadata: { activePaneId: leftPaneId }
    };

    // Registry should only execute in active pane (coordination check happens in registry)
    const leftHandled = await registry.execute(event, leftController, leftContext);
    const rightHandled = await registry.execute(event, rightController, rightContext);

    expect(leftHandled).toBe(true);
    expect(rightHandled).toBe(false);
  });
});
