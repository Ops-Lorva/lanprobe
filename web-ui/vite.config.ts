import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath, URL } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url));
const repoRoot = fileURLToPath(new URL('..', import.meta.url));

// Cible du proxy en développement : le hub tourne en local (conteneur ou
// `cargo run`). Surchargeable par LANPROBE_HUB pour taper une instance distante.
const hubTarget = process.env.LANPROBE_HUB || 'http://127.0.0.1:8088';

export default defineConfig({
  root: here,

  // Chemins relatifs : le hub peut être monté derrière un reverse proxy sur un
  // sous-chemin. Le routage étant en hash (#/probes/…), la profondeur d'URL ne
  // change jamais — les assets relatifs résolvent donc toujours.
  base: './',

  plugins: [
    svelte({
      // Sans ce configFile explicite, le plugin remonterait sur le
      // svelte.config.js racine, qui embarque la config SvelteKit du desktop.
      configFile: `${here}svelte.config.js`,
    }),
  ],

  resolve: {
    alias: {
      // Composants réellement partagés avec l'app desktop (LogoMark, Icons,
      // Sparkline). Une seule source pour la marque et les pictogrammes.
      $desktop: `${repoRoot}src/lib`,
      $app: `${repoRoot}src`,
      $lib: `${here}src/lib`,
    },
  },

  server: {
    port: 5183,
    strictPort: false,
    // Nécessaire pour servir en dev les fichiers importés hors de web-ui/
    // (src/app.css et les composants partagés).
    fs: { allow: [repoRoot] },
    proxy: {
      '/api': { target: hubTarget, changeOrigin: true },
    },
  },

  build: {
    outDir: 'dist',
    emptyOutDir: true,
    // Le backend embarque ce dossier ; pas de sourcemaps en production.
    sourcemap: false,
    target: 'es2022',
  },
});
