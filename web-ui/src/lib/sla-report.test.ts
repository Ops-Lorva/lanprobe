import { describe, expect, it } from 'vitest';
import {
  byPublicIp,
  outages,
  stats,
  windowLabel,
  type PublicIpInterval,
  type Sample,
} from './sla-report';

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

const interval = (ip: string, from: number, to: number): PublicIpInterval => ({
  public_ip: ip,
  interface: 'en0',
  gateway: '10.6.8.1',
  local_subnet: '10.6.8.42',
  confirmed_from: from,
  confirmed_until: to,
  label: null,
});

const sample = (timestamp: number, state: string): Sample => ({
  timestamp,
  alive: state === 'online',
  state,
});

describe('byPublicIp', () => {
  it('impute une coupure suivie du retour sur la MÊME adresse à cette adresse', () => {
    // L'intervalle couvre la coupure : elle appartient bien à cette adresse.
    const rows = byPublicIp(
      [sample(10, 'online'), sample(20, 'offline'), sample(30, 'online')],
      [interval('88.120.0.1', 10, 30)],
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].public_ip).toBe('88.120.0.1');
    expect(rows[0].samples).toBe(3);
    expect(rows[0].uptime_pct).toBeCloseTo(66.67, 1);
  });

  it("n'impute à personne une coupure suivie d'une adresse DIFFÉRENTE", () => {
    // Le trou entre deux intervalles est la zone indéterminée : c'est tout
    // l'intérêt de fermer sur `confirmed_until`.
    const rows = byPublicIp(
      [sample(10, 'online'), sample(20, 'offline'), sample(30, 'online')],
      [interval('88.120.0.1', 10, 10), interval('172.58.0.9', 30, 30)],
    );
    const indetermine = rows.find((r) => r.public_ip === null);
    expect(indetermine?.samples).toBe(1);
    expect(rows.find((r) => r.public_ip === '88.120.0.1')?.uptime_pct).toBe(100);
    expect(rows.find((r) => r.public_ip === '172.58.0.9')?.uptime_pct).toBe(100);
  });

  it('range tout en indéterminé quand aucun intervalle ne couvre la fenêtre', () => {
    // Cas du lendemain de la migration : l'historique n'existe pas encore.
    const rows = byPublicIp([sample(10, 'online'), sample(20, 'online')], []);
    expect(rows).toHaveLength(1);
    expect(rows[0].public_ip).toBeNull();
    expect(rows[0].samples).toBe(2);
  });

  it('regroupe deux passages sur la même adresse en une seule ligne', () => {
    const rows = byPublicIp(
      [sample(10, 'online'), sample(50, 'online')],
      [
        interval('88.120.0.1', 10, 10),
        interval('172.58.0.9', 20, 30),
        interval('88.120.0.1', 50, 50),
      ],
    );
    expect(rows.filter((r) => r.public_ip === '88.120.0.1')).toHaveLength(1);
    expect(rows.find((r) => r.public_ip === '88.120.0.1')?.samples).toBe(2);
  });

  it('ne renvoie pas de ligne indéterminée quand il n’y a rien dedans', () => {
    const rows = byPublicIp([sample(10, 'online')], [interval('88.120.0.1', 0, 100)]);
    expect(rows.some((r) => r.public_ip === null)).toBe(false);
  });
});
