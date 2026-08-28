import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

// Config volontairement minimale : le hub est une SPA Vite, pas SvelteKit.
// (Le svelte.config.js de la racine est celui de l'app desktop Tauri.)
export default {
  preprocess: vitePreprocess(),
};
