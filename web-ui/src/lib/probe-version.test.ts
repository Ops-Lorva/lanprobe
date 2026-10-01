import { describe, it, expect } from 'vitest';
import { newestVersion, isBehind } from './probe-version';

/**
 * Une sonde en retard se voit.
 *
 * 🔴 La référence est la version la plus récente **du parc lui-même**, et non
 * celle publiée sur GitHub : un hub auto-hébergé tourne souvent sans accès
 * sortant, et une couleur qui dépendrait d'un appel externe deviendrait fausse
 * — ou absente — exactement là où on en a besoin.
 *
 * ⚠️ Conséquence assumée, et il faut la dire : si TOUT le parc est en retard,
 * rien n'est signalé. Le hub ne compare que ce qu'il voit.
 */
describe('version la plus récente du parc', () => {
  it('compare des nombres, pas du texte', () => {
    // « 2.10.0 » est plus récent que « 2.9.0 », alors qu'il est avant dans
    // l'ordre alphabétique.
    expect(newestVersion(['2.9.0', '2.10.0'])).toBe('2.10.0');
  });

  it('ignore ce qu\'elle ne sait pas lire', () => {
    expect(newestVersion(['2.4.3', null, undefined, '', 'dev'])).toBe('2.4.3');
  });

  it('ne rend rien quand aucune version n\'est lisible', () => {
    expect(newestVersion([null, 'inconnue'])).toBeNull();
  });
});

describe('sonde en retard', () => {
  it('signale celle qui est derrière', () => {
    expect(isBehind('2.3.0', '2.4.3')).toBe(true);
  });

  it('ne signale ni l\'égale ni la plus avancée', () => {
    expect(isBehind('2.4.3', '2.4.3')).toBe(false);
    // ⚠️ Une sonde en avance (build de test) n'est pas « en retard ». Lui
    // mettre la même couleur ferait chercher une mise à jour qui n'existe pas.
    expect(isBehind('2.5.0', '2.4.3')).toBe(false);
  });

  it('ne signale pas ce qu\'elle ne sait pas lire', () => {
    // 🔴 Une version illisible n'est pas une version périmée. Colorer sur un
    // doute enverrait mettre à jour une sonde qui n'en a pas besoin.
    expect(isBehind(null, '2.4.3')).toBe(false);
    expect(isBehind('dev', '2.4.3')).toBe(false);
    expect(isBehind('2.3.0', null)).toBe(false);
  });

  it('compare les trois nombres, dans l\'ordre', () => {
    expect(isBehind('2.4.3', '2.10.0')).toBe(true);
    expect(isBehind('1.9.9', '2.0.0')).toBe(true);
    expect(isBehind('2.4.3', '2.4.10')).toBe(true);
  });
});
