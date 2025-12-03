<!--
  Query Node Component

  Displays query nodes with their description and filter criteria.
  Query nodes are primarily created by AI/MCP, not manually.

  Full query execution and visualization will be implemented in Issue #443.
  This component provides the foundation with navigation support via "open" button.
-->

<script lang="ts">
	import { createEventDispatcher, getContext } from 'svelte';
	import BaseNode from '$lib/design/components/base-node.svelte';
	import { getNavigationService } from '$lib/services/navigation-service';
	import { DEFAULT_PANE_ID } from '$lib/stores/navigation';

	// Get paneId from context (set by PaneContent) - identifies which pane this node is in
	const sourcePaneId = getContext<string>('paneId') ?? DEFAULT_PANE_ID;

	// Props using Svelte 5 runes mode - same interface as BaseNode
	let {
		nodeId,
		nodeType = 'query',
		autoFocus = false,
		content = $bindable(''),
		children = [],
		metadata = {}
	}: {
		nodeId: string;
		nodeType?: string;
		autoFocus?: boolean;
		content?: string;
		children?: string[];
		metadata?: Record<string, unknown>;
	} = $props();

	const dispatch = createEventDispatcher();

	// Track if user is actively typing (hide button during typing)
	let isTyping = $state(false);
	let typingTimer: ReturnType<typeof setTimeout> | undefined;

	function handleTypingStart() {
		isTyping = true;
		// Clear existing timer
		if (typingTimer) clearTimeout(typingTimer);
		// Hide button for 1 second after last keypress
		typingTimer = setTimeout(() => {
			isTyping = false;
		}, 1000);
	}

	// Reset typing state on mouse movement
	function handleMouseMove() {
		if (isTyping) {
			if (typingTimer) clearTimeout(typingTimer);
			isTyping = false;
		}
	}

	/**
	 * Handle open button click to navigate to query viewer
	 */
	async function handleOpenClick(event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();

		const navigationService = getNavigationService();

		// Regular click: Open in dedicated viewer pane (other pane)
		// Modifier keys provide alternative behaviors
		if (event.metaKey || event.ctrlKey) {
			// Cmd+Click: Open in new tab in source pane (where button was clicked)
			await navigationService.navigateToNode(nodeId, true, sourcePaneId);
		} else {
			// Regular click: Open in dedicated viewer pane (creates new pane if needed)
			// Pass sourcePaneId so it opens in the OTHER pane, not based on focus
			await navigationService.navigateToNodeInOtherPane(nodeId, sourcePaneId);
		}
	}

	/**
	 * Forward all events to parent components
	 */
	function forwardEvent<T>(eventName: string) {
		return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
	}
</script>

<!-- Wrap BaseNode with query-specific styling -->
<div
	class="query-node-wrapper"
	class:typing={isTyping}
	onmousemove={handleMouseMove}
	role="group"
	aria-label="Query node"
>
	<BaseNode
		{nodeId}
		{nodeType}
		{autoFocus}
		bind:content
		{children}
		{metadata}
		on:createNewNode={forwardEvent('createNewNode')}
		on:contentChanged={(e) => {
			handleTypingStart(); // Track typing to hide Open button
			dispatch('contentChanged', e.detail);
		}}
		on:indentNode={forwardEvent('indentNode')}
		on:outdentNode={forwardEvent('outdentNode')}
		on:navigateArrow={forwardEvent('navigateArrow')}
		on:combineWithPrevious={forwardEvent('combineWithPrevious')}
		on:deleteNode={forwardEvent('deleteNode')}
		on:focus={forwardEvent('focus')}
		on:blur={forwardEvent('blur')}
		on:nodeReferenceSelected={forwardEvent('nodeReferenceSelected')}
		on:slashCommandSelected={forwardEvent('slashCommandSelected')}
		on:nodeTypeChanged={forwardEvent('nodeTypeChanged')}
	/>

	<!-- Open button (appears on hover) -->
	<button
		class="query-open-button"
		onclick={handleOpenClick}
		type="button"
		aria-label="Open query in dedicated viewer pane (Cmd+Click for new tab in same pane)"
		title="Open query in viewer"
	>
		open
	</button>
</div>

<style>
	/* Query node wrapper - position relative for absolute button positioning */
	.query-node-wrapper {
		position: relative;
		/* width: 100% handled by parent .node-content-wrapper flex child rule */
	}

	/* Open button (top-right, appears on hover) - matches task/code block pattern */
	.query-open-button {
		position: absolute;
		top: 0.25rem;
		right: 0.25rem;
		background: hsl(var(--background));
		border: 1px solid hsl(var(--border));
		color: hsl(var(--foreground));
		padding: 0.25rem 0.5rem;
		border-radius: 0.25rem;
		font-size: 0.75rem;
		cursor: pointer;
		opacity: 0;
		transition: opacity 0.2s ease;
		text-transform: lowercase;
		z-index: 5; /* Below popovers (1001) but above node content */
	}

	/* Show button on hover, but hide while actively typing */
	.query-node-wrapper:hover:not(.typing) .query-open-button {
		opacity: 1;
	}

	/* Hover state for better feedback */
	.query-open-button:hover {
		background: hsl(var(--muted));
	}
</style>
