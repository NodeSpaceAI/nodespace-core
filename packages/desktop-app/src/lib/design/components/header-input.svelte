<!--
  HeaderInput - the borderless page-title input shared by page-level viewers.

  One always-present input (no button/input swap): click and type. Styling lives
  here so BaseNodeViewer and the query viewer cannot drift apart. The component
  is purely presentational and controlled - the parent decides what `value` is
  while focused and what to do on input/blur/Enter.
-->

<script lang="ts">
  let {
    value,
    placeholder = 'Untitled',
    ariaLabel = 'Page title',
    id,
    readonly = false,
    oninput,
    onfocus,
    onblur,
    onkeydown
  }: {
    value: string;
    placeholder?: string;
    ariaLabel?: string;
    id?: string;
    /** Displays a computed (e.g. title-template) value the user cannot edit. */
    readonly?: boolean;
    oninput?: (_value: string) => void;
    onfocus?: () => void;
    onblur?: () => void;
    onkeydown?: (_event: KeyboardEvent) => void;
  } = $props();
</script>

<input
  type="text"
  {id}
  class="header-input"
  class:header-input--readonly={readonly}
  {value}
  {readonly}
  {placeholder}
  aria-label={ariaLabel}
  oninput={(e) => oninput?.(e.currentTarget.value)}
  onfocus={() => onfocus?.()}
  onblur={() => onblur?.()}
  onkeydown={(e) => onkeydown?.(e)}
/>

<style>
  .header-input {
    width: 100%;
    font-size: 2rem;
    font-weight: 500;
    color: hsl(var(--muted-foreground));
    background: transparent;
    border: none;
    outline: none;
    padding: 0;
    margin: 0;
    font-family: inherit;
  }

  .header-input::placeholder {
    color: hsl(var(--muted-foreground) / 0.5);
  }

  .header-input--readonly {
    cursor: default;
    color: hsl(var(--foreground));
  }

  .header-input--readonly::placeholder {
    color: hsl(var(--muted-foreground) / 0.5);
    font-style: italic;
  }
</style>
