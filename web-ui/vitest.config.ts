import { defineConfig, mergeConfig } from 'vitest/config';
import { fileURLToPath, URL } from 'node:url';
import base from './vite.config';

const here = fileURLToPath(new URL('.', import.meta.url));

/**
 * Tests de la couche de normalisation du hub.
 *
 * ⚠️ Ils existent pour une raison précise : trois défauts d'affichage ont
 * traversé la CI le 29/08/2026 — en-têtes de colonnes disparus, graphiques
 * vides malgré des milliers de points en base, second moteur de speedtest
 * jamais tracé. Aucun n'était visible côté Rust, qui rendait pourtant la
 * bonne réponse. Ce qui casse ici, c'est la lecture de cette réponse.
 *
 * On ne monte pas de composants : les défauts étaient dans des fonctions
 * pures (`normalizeCurves`, `platformLabel`, `rangeMillis`), et un test de
 * fonction pure ne dérive pas avec le rendu.
 */
export default mergeConfig(
  base,
  defineConfig({
    test: {
      root: here,
      include: ['src/**/*.test.ts'],
      environment: 'node',
      reporters: 'dot',
    },
  }),
);
