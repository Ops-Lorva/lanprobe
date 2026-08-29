import { describe, expect, it } from 'vitest';
import { normalizeState, seriesColor } from './charts';

describe('normalizeState', () => {
  it('ramène les états écrits par la sonde à un vocabulaire fermé', () => {
    expect(normalizeState('online')).toBe('online');
    expect(normalizeState('up')).toBe('online');
    expect(normalizeState('OFFLINE')).toBe('offline');
    expect(normalizeState('limited')).toBe('limited');
  });

  it('rend « unknown » plutôt que de laisser passer une valeur imprévue', () => {
    // La bande d'état colorie ce qu'elle reçoit : une valeur inconnue qui
    // traverserait donnerait un segment sans couleur, donc invisible.
    expect(normalizeState(null)).toBe('unknown');
    expect(normalizeState('dégradé')).toBe('unknown');
    expect(normalizeState(42)).toBe('unknown');
  });
});

describe('seriesColor', () => {
  it('donne une teinte distincte à chaque courbe d’un même graphique', () => {
    // Deux cibles pinguées, ou deux moteurs de speedtest, doivent se
    // distinguer : la même couleur pour deux courbes rend la légende inutile.
    const five = [0, 1, 2, 3, 4].map(seriesColor);
    expect(new Set(five).size).toBe(5);
  });

  it('boucle au-delà de la palette plutôt que de rendre une couleur vide', () => {
    expect(seriesColor(5)).toBe(seriesColor(0));
    expect(seriesColor(12)).toBe(seriesColor(2));
  });
});
