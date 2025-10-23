<script lang="ts">
  import { Collapsible } from 'bits-ui';
  import * as Select from '$lib/components/ui/select';
  import * as Popover from '$lib/components/ui/popover';
  import { Calendar } from '$lib/components/ui/calendar';
  import { Button } from '$lib/components/ui/button';
  import { CalendarDate } from '@internationalized/date';
  import { tick } from 'svelte';

  let isOpen = $state(true);
  let backlinksOpen = $state(false);
  let theme = $state<'light' | 'dark'>('light');

  let selectedStatus = $state({ value: "IN_PROGRESS", label: "In Progress" });
  let selectedPriority = $state({ value: "HIGH", label: "High" });

  let dueDate = $state(new CalendarDate(2025, 10, 15));
  let startedAt = $state(new CalendarDate(2025, 10, 8));
  let completedAt = $state<CalendarDate | undefined>(undefined);

  // Combobox state for assignee
  let assigneeOpen = $state(false);
  let assigneeValue = $state("alice");
  let assigneeSearch = $state("");

  const assignees = [
    { value: "alice", label: "@alice" },
    { value: "bob", label: "@bob" },
    { value: "charlie", label: "@charlie" }
  ];

  $effect(() => {
    assigneeSearch;
  });

  // Example task node data
  const taskData = {
    title: "Implement Schema-Driven Property UI",
    status: "In Progress",
    dueDate: "Oct 15, 2025",
    startedAt: "Oct 8, 2025",
    completedAt: null,
    priority: "High",
    assignee: "@alice",
    filledFields: 5,
    totalFields: 6
  };

  // Theme toggle with Cmd+\
  function handleKeydown(event: KeyboardEvent) {
    if ((event.metaKey || event.ctrlKey) && event.key === '\\') {
      event.preventDefault();
      theme = theme === 'light' ? 'dark' : 'light';
      document.documentElement.classList.toggle('dark');
    }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

<section class="example-section">
  <h2>Task Node Viewer - Full Canvas Example</h2>
  <p>Complete BaseNodeViewer showing schema-driven properties and children sections:</p>

  <!-- Full Canvas Viewer (matches HTML design mockup structure) -->
  <div style="border: 1px solid hsl(var(--border)); border-radius: var(--radius); padding: 0; background: hsl(var(--background)); margin: 2rem 0; display: flex; flex-direction: column; height: 800px;">
    <!-- Viewer Header (fixed, doesn't scroll) -->
    <div style="flex-shrink: 0; padding: 1rem; border-bottom: 1px solid hsl(var(--border)); background: hsl(var(--background)); border-top-left-radius: var(--radius); border-top-right-radius: var(--radius);">
      <div style="display: flex; align-items: center; gap: 0.75rem;">
      <!-- Task Icon: Checked Circle (24x24 fills container) -->
      <svg width="24" height="24" viewBox="0 0 24 24" fill="none" style="color: hsl(var(--node-task))">
        <!-- Circle: 24px diameter (r=12), fills entire container -->
        <circle cx="12" cy="12" r="12" fill="currentColor"/>
        <!-- Checkmark: proportionally scaled for full 24px circle -->
        <path d="M7.5 12l3.5 3.5 6.5-6.5" stroke="hsl(var(--background))" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/>
      </svg>
      <h1 style="font-size: 2rem; font-weight: 500; color: hsl(var(--muted-foreground)); margin: 0;">{taskData.title}</h1>
      </div>
    </div>

    <!-- Scrollable Content Area -->
    <div style="flex: 1; overflow-y: auto; padding: 0 1.5rem 1.5rem 1.5rem;">
    <!-- SECTION 1: Schema-Driven Properties (Accordion) -->
    <div class="border-b">
      <Collapsible.Root bind:open={isOpen}>
        <Collapsible.Trigger class="flex w-full items-center justify-between py-3 font-medium transition-all hover:opacity-80">
          <div class="flex items-center gap-3">
            <!-- Status Badge -->
            <span class="inline-flex items-center rounded-md border border-border bg-muted px-2.5 py-0.5 text-xs font-medium text-foreground">
              {taskData.status}
            </span>

            <!-- Due Date -->
            <span class="text-sm text-muted-foreground">Due Oct 15</span>
          </div>

          <!-- Field Completion + Chevron -->
          <div class="flex items-center gap-2">
            <span class="text-sm text-muted-foreground">{taskData.filledFields}/{taskData.totalFields} fields</span>
            <svg
              class="h-4 w-4 text-muted-foreground transition-transform duration-200"
              class:rotate-180={isOpen}
              viewBox="0 0 16 16"
              fill="none"
            >
              <path d="M4 6l4 4 4-4" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          </div>
        </Collapsible.Trigger>

        <Collapsible.Content class="pb-4">
          <!-- Property Grid (2 columns) -->
          <div class="grid grid-cols-2 gap-4">
            <!-- Status Field -->
            <div class="space-y-2">
              <label class="text-sm font-medium">Status</label>
              <Select.Root bind:selected={selectedStatus} type="single">
                <Select.Trigger class="w-full">
                  {selectedStatus.label}
                </Select.Trigger>
                <Select.Content>
                  <Select.Item value="OPEN" label="Open" />
                  <Select.Item value="IN_PROGRESS" label="In Progress" />
                  <Select.Item value="DONE" label="Done" />
                  <Select.Item value="BLOCKED" label="Blocked" />
                </Select.Content>
              </Select.Root>
            </div>

            <!-- Priority Field -->
            <div class="space-y-2">
              <label class="text-sm font-medium">Priority</label>
              <Select.Root bind:selected={selectedPriority} type="single">
                <Select.Trigger class="w-full">
                  {selectedPriority.label}
                </Select.Trigger>
                <Select.Content>
                  <Select.Item value="LOW" label="Low" />
                  <Select.Item value="MEDIUM" label="Medium" />
                  <Select.Item value="HIGH" label="High" />
                </Select.Content>
              </Select.Root>
            </div>

            <!-- Due Date Field -->
            <div class="space-y-2">
              <label class="text-sm font-medium">Due Date</label>
              <Popover.Root>
                <Popover.Trigger class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none">
                  <span>{dueDate.toString()}</span>
                  <svg class="h-4 w-4 opacity-50" viewBox="0 0 16 16" fill="none">
                    <rect x="2" y="3" width="12" height="11" rx="1" stroke="currentColor" stroke-width="1.5"/>
                    <path d="M5 1v3M11 1v3M2 6h12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                  </svg>
                </Popover.Trigger>
                <Popover.Content class="w-auto p-0" align="start">
                  <Calendar bind:value={dueDate} selectionMode="single" />
                </Popover.Content>
              </Popover.Root>
            </div>

            <!-- Started At Field -->
            <div class="space-y-2">
              <label class="text-sm font-medium">Started At</label>
              <Popover.Root>
                <Popover.Trigger class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none">
                  <span>{startedAt.toString()}</span>
                  <svg class="h-4 w-4 opacity-50" viewBox="0 0 16 16" fill="none">
                    <rect x="2" y="3" width="12" height="11" rx="1" stroke="currentColor" stroke-width="1.5"/>
                    <path d="M5 1v3M11 1v3M2 6h12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                  </svg>
                </Popover.Trigger>
                <Popover.Content class="w-auto p-0" align="start">
                  <Calendar bind:value={startedAt} selectionMode="single" />
                </Popover.Content>
              </Popover.Root>
            </div>

            <!-- Completed At Field -->
            <div class="space-y-2">
              <label class="text-sm font-medium">Completed At</label>
              <Popover.Root>
                <Popover.Trigger class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none">
                  <span class={completedAt ? "" : "text-muted-foreground"}>
                    {completedAt ? completedAt.toString() : "Pick a date"}
                  </span>
                  <svg class="h-4 w-4 opacity-50" viewBox="0 0 16 16" fill="none">
                    <rect x="2" y="3" width="12" height="11" rx="1" stroke="currentColor" stroke-width="1.5"/>
                    <path d="M5 1v3M11 1v3M2 6h12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                  </svg>
                </Popover.Trigger>
                <Popover.Content class="w-auto p-0" align="start">
                  <Calendar bind:value={completedAt} selectionMode="single" />
                </Popover.Content>
              </Popover.Root>
            </div>

            <!-- Assignee Field (Combobox) -->
            <div class="space-y-2">
              <label class="text-sm font-medium">Assignee</label>
              <Popover.Root bind:open={assigneeOpen}>
                <Popover.Trigger class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none">
                  <span>{assignees.find(a => a.value === assigneeValue)?.label ?? "Select assignee..."}</span>
                  <svg class="h-4 w-4 opacity-50" viewBox="0 0 16 16" fill="none">
                    <path d="M4 6l4 4 4-4" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                  </svg>
                </Popover.Trigger>
                <Popover.Content class="w-[200px] p-0" align="start">
                  <div class="flex flex-col">
                    <input
                      type="text"
                      placeholder="Search assignee..."
                      bind:value={assigneeSearch}
                      class="flex h-10 w-full border-b border-input bg-background px-3 py-2 text-sm focus-visible:outline-none"
                    />
                    <div class="max-h-[200px] overflow-y-auto">
                      {#each assignees.filter(a => a.label.toLowerCase().includes(assigneeSearch.toLowerCase())) as assignee}
                        <button
                          type="button"
                          class="relative flex w-full cursor-pointer select-none items-center rounded-sm px-3 py-2 text-sm outline-none hover:bg-muted"
                          onclick={() => {
                            assigneeValue = assignee.value;
                            assigneeOpen = false;
                            assigneeSearch = "";
                          }}
                        >
                          {assignee.label}
                          {#if assigneeValue === assignee.value}
                            <svg class="ml-auto h-4 w-4" viewBox="0 0 16 16" fill="none">
                              <path d="M3 8l4 4 6-8" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                            </svg>
                          {/if}
                        </button>
                      {/each}
                    </div>
                  </div>
                </Popover.Content>
              </Popover.Root>
            </div>
          </div>
        </Collapsible.Content>
      </Collapsible.Root>
    </div>

    <!-- SECTION 2: Children -->
    <div class="pt-2 pb-4">
      <div class="hierarchy-example">
        <!-- Implementation Tasks (Parent) -->
        <div class="hierarchy-node has-children">
          <span class="chevron-icon expanded">
            <svg viewBox="0 0 16 16">
              <path d="M6 3l5 5-5 5-1-1 4-4-4-4 1-1z"/>
            </svg>
          </span>
          <div class="hierarchy-indicator">
            <div class="node-icon">
              <div class="node-ring"></div>
              <div class="text-circle"></div>
            </div>
          </div>
          Implementation Tasks
        </div>

        <!-- Child tasks -->
        <div class="hierarchy-node child-1">
          <div class="hierarchy-indicator">
            <div class="task-icon">
              <div class="task-circle task-circle-completed"></div>
            </div>
          </div>
          <span style="text-decoration: line-through; opacity: 0.7;">Create SchemaPropertyForm component</span>
        </div>

        <div class="hierarchy-node child-1">
          <div class="hierarchy-indicator">
            <div class="task-icon">
              <div class="task-circle task-circle-in-progress"></div>
            </div>
          </div>
          Implement field type mapping service
        </div>

        <div class="hierarchy-node child-1">
          <div class="hierarchy-indicator">
            <div class="task-icon">
              <div class="task-circle task-circle-pending"></div>
            </div>
          </div>
          Add validation for required fields
        </div>

        <div class="hierarchy-node child-1">
          <div class="hierarchy-indicator">
            <div class="task-icon">
              <div class="task-circle task-circle-pending"></div>
            </div>
          </div>
          Write tests for property renderer
        </div>
      </div>
    </div>

    <!-- SECTION 3: Backlinks (Mock) -->
    <div class="border-t pt-2">
      <Collapsible.Root bind:open={backlinksOpen}>
        <Collapsible.Trigger class="flex w-full items-center justify-between py-3 font-medium transition-all hover:opacity-80">
          <div class="flex items-center gap-3">
            <span class="text-sm text-muted-foreground">
              Mentioned by: (3 nodes)
            </span>
          </div>

          <div class="flex items-center gap-2">
            <svg
              viewBox="0 0 16 16"
              fill="none"
              class="h-4 w-4 text-muted-foreground transition-transform duration-200"
              class:rotate-180={backlinksOpen}
            >
              <path
                d="M4 6l4 4 4-4"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            </svg>
          </div>
        </Collapsible.Trigger>

        <Collapsible.Content>
          <div class="pb-4">
            <ul class="flex flex-col gap-1">
              <!-- Date backlink -->
              <li>
                <a
                  href="#"
                  class="flex items-center gap-2 px-2 py-1.5 text-sm no-underline"
                >
                  <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                    <rect x="2" y="3" width="12" height="11" rx="1" stroke="currentColor" stroke-width="1.5"/>
                    <path d="M5 1v3M11 1v3M2 6h12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                  </svg>
                  <span class="flex-1 truncate">
                    2025-10-14 - Sprint Planning
                  </span>
                </a>
              </li>

              <!-- Another date backlink -->
              <li>
                <a
                  href="#"
                  class="flex items-center gap-2 px-2 py-1.5 text-sm no-underline"
                >
                  <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                    <rect x="2" y="3" width="12" height="11" rx="1" stroke="currentColor" stroke-width="1.5"/>
                    <path d="M5 1v3M11 1v3M2 6h12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                  </svg>
                  <span class="flex-1 truncate">
                    2025-10-12 - Design System Review
                  </span>
                </a>
              </li>

              <!-- Task backlink -->
              <li>
                <a
                  href="#"
                  class="flex items-center gap-2 px-2 py-1.5 text-sm no-underline"
                >
                  <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                    <circle cx="8" cy="8" r="7" stroke="currentColor" stroke-width="1.5" fill="none"/>
                  </svg>
                  <span class="flex-1 truncate">
                    Review property form implementation
                  </span>
                </a>
              </li>
            </ul>
          </div>
        </Collapsible.Content>
      </Collapsible.Root>
    </div>
  </div>
  </div>
</section>

<style>
  .example-section {
    margin-bottom: 32px;
    padding: 24px;
    border: 1px solid hsl(var(--border));
    border-radius: var(--radius);
    background: hsl(var(--card));
  }

  .example-section h2 {
    font-size: 32px;
    font-weight: 600;
    margin: 0 0 16px 0;
    color: hsl(var(--foreground));
  }

  .example-section p {
    font-size: 16px;
    color: hsl(var(--foreground));
    margin: 16px 0;
  }
</style>
