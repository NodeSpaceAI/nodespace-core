// Tauri doesn't have a Node.js server to do proper SSR
// so we will use adapter-static to prerender the app (SSG)
// See: https://v2.tauri.app/start/frontend/sveltekit/ for more info
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess({
    // Use PostCSS for stable CSS processing
    style: {
      postcss: true
    }
  }),
  kit: {
    adapter: adapter(),
    // The host API for build-time extensions. SvelteKit writes the alias into its
    // generated tsconfig, so svelte-check resolves it and its subpaths too.
    alias: {
      "@nodespace/extension-api": "src/lib/extension-api",
    },
  },
};

export default config;
