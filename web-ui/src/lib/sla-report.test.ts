import { describe, expect, it } from 'vitest';
import { outages, stats, windowLabel, type Sample } from './sla-report';

const t = (k: string, v?: Record<string, string | number>) =>
  v ? `${k}:${Object.values(v).join(',')}` : k;

function series(pattern: string, from = 1_788_000_000): Sample[] {
  // 'o' = répond, 'x' = ne répond pas. Un relevé par seconde.
  return [...pattern].map((c, i) => ({
    timestamp: from + i,
    alive: c === 'o',
    latency_ms: c === 'o' ? 10 + i : null,
  }));
}

describe('outages', () => {
  it('isole chaque coupure, avec ce qu’elle a coûté', () => {
    const list = outages(series('ooxxoooxo'));
    expect(list).toHaveLength(2);
    expect(list[0].samples_lost).toBe(2);
    expect(list[1].samples_lost).toBe(1);
  });

  it('laisse une coupure en cours ouverte plutôt que d’inventer sa fin', () => {
    // ⚠️ La fermer sur l'instant de l'export fabriquerait une heure de
    // rétablissement qui n'a pas eu lieu — dans un document remis au client.
    const list = outages(series('ooxx'));
    expect(list).toHaveLength(1);
    expect(list[0].end).toBeNull();
  });

  it('ne voit aucune coupure sur une série saine', () => {
    expect(outages(series('oooo'))).toEqual([]);
    expect(outages([])).toEqual([]);
  });
});

describe('stats', () => {
  it('calcule la disponibilité sur « répond », pas sur la latence', () => {
    // Le piège : un hôte muet n'écrit pas de latence. Compter les latences
    // donnerait 100 % sur un hôte mort — l'inverse de la vérité.
    const dead: Sample[] = [
      { timestamp: 1, alive: false },
      { timestamp: 2, alive: false },
    ];
    expect(stats(dead).uptime_pct).toBe(0);
    expect(stats(dead).failed).toBe(2);
  });

  it('rend une disponibilité exacte sur une série mixte', () => {
    expect(stats(series('ooox')).uptime_pct).toBe(75);
  });

  it('ne prétend rien sur une série vide', () => {
    const s = stats([]);
    expect(s.total).toBe(0);
    expect(s.avg).toBeNull();
    expect(s.p95).toBeNull();
  });

  it('rend min, max et P95 depuis les latences observées', () => {
    const s = stats(series('ooooo'));
    expect(s.min).toBe(10);
    expect(s.max).toBe(14);
    expect(s.p95).not.toBeNull();
  });
});

describe('windowLabel', () => {
  it('écrit la période en toutes lettres', () => {
    // « 99,2 % » ne veut rien dire sans savoir sur quoi.
    expect(windowLabel('-24h', t)).toBe('sla.window_hours:24');
    expect(windowLabel('-7d', t)).toBe('sla.window_days:7');
  });

  it('laisse passer une fenêtre qu’elle ne sait pas nommer', () => {
    expect(windowLabel('-90m', t)).toBe('-90m');
  });
});

describe('windowLabel avec bornes explicites', () => {
  it('écrit « du … au … » plutôt qu’une durée relative', () => {
    // ⚠️ Un rapport contractuel porte sur une période convenue. « 7 derniers
    // jours » donnerait un chiffre différent à chaque ouverture du document.
    const out = windowLabel('1788000000..1788600000', t, 'fr');
    expect(out).toContain('sla.from');
    expect(out).toContain('sla.to');
    expect(out).not.toContain('window_days');
  });
});
