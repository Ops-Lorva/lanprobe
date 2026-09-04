import { readFileSync } from 'node:fs';
import { fileURLToPath, URL } from 'node:url';
import { describe, expect, it } from 'vitest';

/**
 * La couleur du **mode terrain armé**, sur le hub comme dans l'app.
 *
 * 🔴 Le repère d'un état ne se négocie pas avec le thème. Le hub laisse choisir
 * six palettes (indigo, cyan, emerald, rose, amber, slate) et le temps réel
 * portait `--ep-accent` : en palette « emerald » il prenait le vert de « en
 * ligne », en « rose » le rouge de « hors ligne », en « amber » l'orange de
 * l'avertissement. Un état repeint par le goût de l'utilisateur ne dit plus
 * rien — c'est le raisonnement déjà tenu pour les couleurs de séries.
 *
 * ⚠️ Et c'est un ÉTAT, pas un lien : lui laisser l'accent des éléments
 * interactifs le faisait aussi ressembler à quelque chose qui se clique.
 */

const lire = (rel: string) =>
  readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');

const WEB_CSS = lire('../web.css');
const APP_CSS = lire('../../../src/app.css');

function luminance(hex: string): number {
  const v = hex.replace('#', '');
  const canal = (i: number) => {
    const c = parseInt(v.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * canal(0) + 0.7152 * canal(2) + 0.0722 * canal(4);
}

function contraste(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

/** Toutes les valeurs déclarées pour une variable, dans l'ordre du fichier. */
function declarations(css: string, name: string): string[] {
  return [...css.matchAll(new RegExp(`--${name}:\\s*([^;]+);`, 'g'))].map((m) => m[1].trim());
}

/**
 * Les fonds du hub, lus dans `app.css` : toute palette ajoutée demain est
 * couverte sans qu'on y pense. Classés par leur propre clarté — un fond sombre
 * porte du texte clair, et réciproquement.
 */
function fonds(): { sombres: string[]; clairs: string[] } {
  const tous = [
    ...new Set(
      [...APP_CSS.matchAll(/--ep-bg-(?:primary|secondary|tertiary):\s*(#[0-9a-f]{6})/gi)].map((m) =>
        m[1].toLowerCase(),
      ),
    ),
  ];
  return {
    sombres: tous.filter((c) => luminance(c) < 0.2),
    clairs: tous.filter((c) => luminance(c) >= 0.2),
  };
}

/** Le corps de la règle CSS d'un sélecteur, dans un composant Svelte. */
function regle(fichier: string, selecteur: string): string {
  const css = lire(fichier);
  const i = css.indexOf(`\n  ${selecteur} {`);
  expect(i, `sélecteur « ${selecteur} » introuvable dans ${fichier}`).toBeGreaterThan(-1);
  return css.slice(i, css.indexOf('}', i));
}

describe('la couleur de l’armé', () => {
  const tons = declarations(WEB_CSS, 'lp-armed');

  // ⚠️ Trois blocs, comme les couleurs de séries : `:root` (sombre),
  // `[data-theme='light']`, et le clair du système.
  it('est déclarée pour le sombre comme pour le clair', () => {
    expect(tons.length).toBe(3);
    expect(new Set(tons).size).toBe(2);
  });

  // 🔴 Le cœur de la règle : une valeur littérale. Pointer `--ep-accent`
  // rendrait de nouveau l'état repeignable par la palette.
  it('ne renvoie à aucune variable de palette', () => {
    for (const ton of tons) {
      expect(ton, 'la couleur d’un état est fixe').toMatch(/^#[0-9a-f]{6}$/i);
    }
  });

  // Calculés à l'appel : un `reduce` au chargement ferait tomber la suite
  // entière sur une variable absente, au lieu de dire laquelle manque.
  const plusClair = () => tons.reduce((a, b) => (luminance(a) > luminance(b) ? a : b), '#000000');
  const plusSombre = () => tons.reduce((a, b) => (luminance(a) < luminance(b) ? a : b), '#ffffff');

  /**
   * 🔴 La cohérence entre surfaces, qui est la demande. L'app iOS porte
   * `Palette.armed = "#818cf8"` — elle l'avait elle-même pris du hub, sous le
   * nom `--ep-accent-bright`. On fige donc la même valeur des deux côtés, au
   * lieu de la laisser dépendre de la palette choisie sur le hub.
   */
  it('est exactement celle de l’éclair de l’app sur fond sombre', () => {
    expect(plusClair().toLowerCase()).toBe('#818cf8');
  });

  it('porte du texte sur TOUS les fonds du hub, palettes comprises', () => {
    const [clair, sombre] = [plusClair(), plusSombre()];
    const { sombres, clairs } = fonds();
    expect(sombres.length).toBeGreaterThan(4);
    expect(clairs.length).toBeGreaterThan(2);
    for (const fond of sombres) {
      expect(contraste(clair, fond), `${clair} sur ${fond}`).toBeGreaterThanOrEqual(4.5);
    }
    for (const fond of clairs) {
      expect(contraste(sombre, fond), `${sombre} sur ${fond}`).toBeGreaterThanOrEqual(4.5);
    }
  });
});

/**
 * ⚠️ La variable ne sert à rien si les repères continuent de lire l'accent.
 * Ces trois règles SONT le mode terrain sur le hub : le fanion du parc, le
 * bouton armé de la fiche, et la ligne qui dit le temps restant.
 */
describe('les repères du temps réel', () => {
  const REPERES: [string, string][] = [
    ['./components/ProbeRow.svelte', '.rtflag'],
    ['../views/ProbeView.svelte', '.rt.on'],
    ['../views/ProbeView.svelte', '.rt-note'],
  ];

  for (const [fichier, selecteur] of REPERES) {
    it(`${selecteur} porte le bleu de l’armé, pas l’accent`, () => {
      const corps = regle(fichier, selecteur);
      expect(corps).toContain('--lp-armed');
      expect(corps, 'un état ne se peint pas avec l’accent des liens').not.toContain('--ep-accent');
    });
  }

  // ⚠️ Le temps restant reste écrit sur le hub. « Juste la couleur » est la
  // règle de l'app mobile, qui est sèche ; le hub a droit à un peu plus de
  // texte, et le retirer ferait perdre une information que personne ne
  // demandait à perdre.
  it('gardent le temps restant, que l’app mobile n’affiche pas', () => {
    expect(lire('./components/ProbeRow.svelte')).toContain('fleet.realtime_flag');
    expect(lire('../views/ProbeView.svelte')).toContain('probe.realtime_active');
  });
});
