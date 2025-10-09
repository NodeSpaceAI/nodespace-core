<script lang="ts">
  import SchemaPropertyFormDemo from './lib/SchemaPropertyFormDemo.svelte';
</script>

<div class="style-guide">
  <header>
    <h1>Schema-Driven Property Forms</h1>
    <p>Automatic property UI generation based on schema definitions for extensible node types.</p>
  </header>

  <main>
    <SchemaPropertyFormDemo />
  </main>
</div>

<style>
  .style-guide {
    min-height: 100vh;
    background: hsl(var(--background));
    max-width: 1200px;
    padding: 32px;
    margin: 0 auto;
  }

  header {
    padding: 0;
  }

  header h1 {
    font-size: 40px;
    font-weight: 700;
    margin: 26.8px 0 16px 0;
    color: hsl(var(--foreground));
  }

  header p {
    font-size: 16px;
    color: hsl(var(--muted-foreground));
    margin: 16px 0;
  }

  main {
    padding: 0;
  }
</style>
