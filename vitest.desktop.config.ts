import { defineConfig } from 'vitest/config';

/**
 * Tests de l'app desktop. Fonctions pures et fichiers lus tels quels : pas de
 * composant monté, pour la même raison que côté hub (`web-ui/vitest.config.ts`)
 * — un test de fonction ne dérive pas avec le rendu.
 */
export default defineConfig({
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
    reporters: 'dot',
  },
});
