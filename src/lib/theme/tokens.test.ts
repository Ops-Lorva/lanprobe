import { readFileSync } from 'node:fs';
import { fileURLToPath, URL } from 'node:url';
import { describe, expect, it } from 'vitest';

/**
 * Les tokens de la DA « Relevé » dans `src/app.css`, partagé avec le hub.
 *
 * Les valeurs viennent de la maquette validée
 * (`design/lanprobe-da-releve/lanprobe-da/tokens.css`), qui n'est pas dans ce
 * dépôt : elles sont recopiées ici pour qu'une retouche de couleur se voie.
 */
const lire = (rel: string) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');
const APP_CSS = lire('../../app.css');

/** Le corps du premier bloc dont le sélecteur est exactement `selecteur`. */
function bloc(css: string, selecteur: string): string {
  const i = css.indexOf(`${selecteur} {`);
  expect(i, `bloc « ${selecteur} » introuvable`).toBeGreaterThan(-1);
  return css.slice(i, css.indexOf('}', i));
}
function valeur(corps: string, nom: string): string | undefined {
  return new RegExp(`--${nom}:\\s*([^;]+);`).exec(corps)?.[1].trim();
}

describe('la police', () => {
  // 🔴 Le bug : app.css demandait `'Geist'`, le paquet déclare `'Geist Sans'`.
  // Aucune police ne correspondait, l'app tombait sur la police système.
  it('nomme en premier la famille que déclare @fontsource/geist-sans', () => {
    const fontsource = lire('../../../node_modules/@fontsource/geist-sans/400.css');
    const famille = /font-family:\s*'([^']+)'/.exec(fontsource)?.[1];
    expect(famille).toBe('Geist Sans');
    for (const nom of ['ep-font-sans', 'font-body']) {
      const decl = valeur(APP_CSS, nom) ?? '';
      expect(decl.split(',')[0].trim().replace(/['"]/g, ''), `--${nom}`).toBe(famille);
    }
  });
});

describe('les tokens Relevé', () => {
  const SOMBRE = {
    'color-paper': '#080c14', 'color-paper-2': '#0d1220', 'color-ink': '#e8eaf0',
    'color-muted': '#8892a4', 'color-rule': '#1c2434', 'color-accent': '#4f46e5',
    'color-accent-text': '#a5b4fc', 'st-ok': '#22c55e', 'st-warn': '#f59e0b', 'st-bad': '#f87171',
    'lp-series-1': '#6366f1', 'lp-series-2': '#14a3bd', 'lp-series-3': '#cf6f50', 'lp-armed': '#818cf8',
  };
  const CLAIR = {
    'color-paper': '#f3f5f9', 'color-paper-2': '#ffffff', 'color-ink': '#0f1220',
    'color-muted': '#5b6578', 'color-rule': '#dde2ec', 'color-accent': '#4f46e5',
    'color-accent-text': '#4338ca', 'st-ok': '#15803d', 'st-warn': '#b45309', 'st-bad': '#c42020',
    'lp-series-1': '#4f46e5', 'lp-series-2': '#0891b2', 'lp-series-3': '#c2410c', 'lp-armed': '#4f46e5',
  };
  const BLOCS: [string, Record<string, string>][] = [
    [':root', SOMBRE],
    ["[data-theme='dark']", SOMBRE],
    ["[data-theme='light']", CLAIR],
    // Le clair du système : seulement quand aucun thème n'est forcé.
    [':root:not([data-theme])', CLAIR],
  ];
  for (const [sel, attendu] of BLOCS) {
    it(`${sel} porte les valeurs de la maquette`, () => {
      const corps = bloc(APP_CSS, sel);
      for (const [nom, v] of Object.entries(attendu)) {
        expect(valeur(corps, nom)?.toLowerCase(), `--${nom} dans ${sel}`).toBe(v);
      }
    });
  }
});
