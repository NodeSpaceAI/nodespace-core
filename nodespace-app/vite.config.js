import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";

const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig(async () => ({
  plugins: [sveltekit()],
  
  // Fix esbuild CSS processing issues
  esbuild: {
    keepNames: true,
    // Use a safer target that doesn't trigger esbuild service crashes
    target: 'es2020',
    // Prevent esbuild service crashes during style processing
    loader: { '.css': 'css' }
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
