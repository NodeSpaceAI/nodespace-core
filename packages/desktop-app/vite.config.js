import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";

const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig(async () => ({
  plugins: [sveltekit()],

  // Fix esbuild CSS processing issues
  esbuild: {
    keepNames: true,
    // Use es2022 to preserve native class fields for Svelte 5 $state() runes
    // Svelte compiler needs to process $state() BEFORE esbuild transforms class fields
    target: 'es2022'
  },

  // CSS configuration for stable processing
  css: {
    postcss: './postcss.config.js',
    // Prevent transformer errors
    transformer: 'postcss'
  },

  // Common Tauri/Svelte optimization settings
  optimizeDeps: {
    // Exclude problematic dependencies that don't play well with pre-bundling
    exclude: [
      '@tauri-apps/api',
      '@tauri-apps/plugin-opener'
    ],
    // Include dependencies that should be pre-bundled
    include: ['uuid', 'clsx', 'tailwind-merge']
  },

  // Build configuration
  build: {
    // Suppress the "dynamic/static import mixing" warnings
    // These are intentional: Tauri APIs are dynamically imported for lazy loading
    // while also being statically imported by other modules. This is expected
    // behavior for environment-adaptive code and doesn't affect bundle output.
    // Note: Vite 6's reporter plugin may still show some warnings that can't be
    // suppressed via onwarn - these are informational only and don't affect the build.
    rollupOptions: {
      onwarn(warning, warn) {
        // Suppress dynamic/static import mixing warnings
        // This pattern is intentional for environment-adaptive code (Tauri/browser)
        if (warning.message?.includes('is dynamically imported by') &&
            warning.message?.includes('but also statically imported by')) {
          return;
        }
        // Suppress circular dependency warnings for internal modules
        if (warning.code === 'CIRCULAR_DEPENDENCY') {
          return;
        }
        warn(warning);
      },
      output: {
        // Split large vendor chunks to stay under 500KB limit
        // This improves initial load time through better caching
        manualChunks(id) {
          if (id.includes('node_modules')) {
            // Tauri APIs - separate chunk for desktop-specific code
            if (id.includes('@tauri-apps')) {
              return 'vendor-tauri';
            }
            // UI component libraries - separate chunk for lazy-loadable UI
            if (id.includes('bits-ui') || id.includes('@lucide') || id.includes('sveltednd')) {
              return 'vendor-ui';
            }
            // All other node_modules (including svelte) in single vendor chunk
            // to avoid circular chunk dependencies
            return 'vendor';
          }
        }
      }
    }
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
