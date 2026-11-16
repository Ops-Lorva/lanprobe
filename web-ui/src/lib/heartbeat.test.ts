import { describe, expect, it } from 'vitest';
import { gatewayLoss, gatewayLatency, heartbeatCells, lossLevel, lossCells } from './heartbeat';

const H = 3_600_000;
const TO = 1_788_015_600_000; // aligné sur un quart d'heure
const FROM = TO - 24 * H;
const Q = 15 * 60_000;
const s = (ms: number) => ms / 1000;

/** Un relevé d'état internet toutes les minutes sur [a, b). */
function states(a: number, b: number, v: string) {
  const out: { t: number; v: string }[] = [];
  for (let t = a; t < b; t += 60_000) out.push({ t, v });
  return out;
}

describe('heartbeatCells — la bande de battement 24 h', () => {
  it('rend 96 quarts d’heure, tous calmes quand la sonde bat normalement', () => {
    const cells = heartbeatCells({
      points: states(FROM, TO, 'online'),
      from: FROM,
      to: TO,
      createdAt: s(FROM - 10 * H),
      lastSeen: s(TO - 5_000),
      atMs: TO,
    });
    expect(cells).toHaveLength(96);
    expect(cells.every((c) => c === 'ok')).toBe(true);
  });

  it('colore en orange un quart d’heure où internet a été limité, en rouge s’il a été coupé', () => {
    const pts = [
      ...states(FROM, TO, 'online'),
      { t: FROM + 10 * Q + 60_000, v: 'limited' },
      { t: FROM + 20 * Q + 60_000, v: 'offline' },
    ];
    const cells = heartbeatCells({
      points: pts,
      from: FROM,
      to: TO,
      createdAt: s(FROM - H),
      lastSeen: s(TO),
      atMs: TO,
    });
    expect(cells[10]).toBe('w');
    expect(cells[20]).toBe('b');
    expect(cells[11]).toBe('ok');
  });

  it('hachure ce qui précède l’enrôlement et les trous sans relevé', () => {
    const created = FROM + 12 * H;
    const pts = [...states(created, created + 2 * H, 'online'), ...states(created + 3 * H, TO, 'online')];
    const cells = heartbeatCells({
      points: pts,
      from: FROM,
      to: TO,
      createdAt: s(created),
      lastSeen: s(TO),
      atMs: TO,
    });
    expect(cells[0]).toBe('n');
    expect(cells[47]).toBe('n');
    expect(cells[48]).toBe('ok');
    // Le trou d'une heure entre 14 h et 15 h après l'enrôlement : on ne sait
    // pas, on ne l'affirme pas rouge.
    expect(cells[48 + 9]).toBe('n');
  });

  it('peint en rouge le silence en cours d’une sonde hors ligne, depuis son dernier battement', () => {
    const last = TO - 3 * H;
    const cells = heartbeatCells({
      points: states(FROM, last, 'online'),
      from: FROM,
      to: TO,
      createdAt: s(FROM - H),
      lastSeen: s(last),
      atMs: TO,
    });
    expect(cells.slice(-12).every((c) => c === 'b')).toBe(true);
    expect(cells[96 - 13]).toBe('ok');
  });

  it('ne peint PAS en rouge le silence d’une sonde seulement silencieuse', () => {
    const last = TO - 20 * 60_000;
    const cells = heartbeatCells({
      points: states(FROM, last, 'online'),
      from: FROM,
      to: TO,
      createdAt: s(FROM - H),
      lastSeen: s(last),
      atMs: TO,
    });
    expect(cells[95]).toBe('n');
  });

  it('marque le dernier quart d’heure en temps réel quand le mode tourne', () => {
    const cells = heartbeatCells({
      points: states(FROM, TO, 'online'),
      from: FROM,
      to: TO,
      createdAt: s(FROM - H),
      lastSeen: s(TO),
      atMs: TO,
      realtimeUntil: s(TO + 10 * 60_000),
    });
    expect(cells[95]).toBe('a');
    expect(cells[94]).toBe('ok');
  });

  it('hachure tout ce qui est sans relevé pour une sonde jamais vue', () => {
    const cells = heartbeatCells({
      points: [],
      from: FROM,
      to: TO,
      createdAt: s(TO - H),
      lastSeen: null,
      atMs: TO,
    });
    expect(cells.every((c) => c === 'n')).toBe(true);
  });
});

describe('lossLevel — la perte en niveaux de gris', () => {
  it('suit les seuils de la maquette : 0, < 1 %, 1 à 5 %, 5 à 20 %, > 20 %', () => {
    expect(lossLevel(0)).toBe(0);
    expect(lossLevel(0.5)).toBe(1);
    expect(lossLevel(1)).toBe(2);
    expect(lossLevel(4.9)).toBe(2);
    expect(lossLevel(5)).toBe(3);
    expect(lossLevel(20)).toBe(3);
    expect(lossLevel(20.1)).toBe(4);
    expect(lossLevel(100)).toBe(4);
  });
});

describe('lossCells — la rangée de perte sous la courbe d’une cible', () => {
  it('calcule la perte de chaque case depuis la disponibilité `alive` (1 = répond)', () => {
    const pts = [
      { t: FROM + 1000, v: 1 },
      { t: FROM + 2000, v: 0 },
      { t: FROM + Q + 1000, v: 1 },
    ];
    const cells = lossCells(pts, FROM, TO, 96);
    expect(cells).toHaveLength(96);
    expect(cells[0]).toBe(50);
    expect(cells[1]).toBe(0);
    // Pas de relevé : pas de case, plutôt qu'un 0 % qui affirmerait.
    expect(cells[2]).toBeNull();
  });

  it('prend aussi les moyennes agrégées par le hub, qui sont déjà des taux', () => {
    const cells = lossCells([{ t: FROM + 1000, v: 0.98 }], FROM, TO, 96);
    expect(cells[0]).toBeCloseTo(2, 5);
  });
});

describe('passerelle — latence et perte de la ligne du parc', () => {
  it('lit la latence de la cible qui est la passerelle, sinon de la première', () => {
    const monitors = [
      { ip: '1.1.1.1', alive: true, latency_ms: 19, uptime_pct: 100, samples: 1 },
      { ip: '192.168.1.1', alive: true, latency_ms: 1.2, avg_ms: 1.1, uptime_pct: 100, samples: 1 },
    ];
    expect(gatewayLatency({ gateway: '192.168.1.1', monitors })).toBe(1.1);
    expect(gatewayLatency({ gateway: null, monitors })).toBe(19);
    expect(gatewayLatency({ gateway: null, monitors: [] })).toBeNull();
  });

  it('lit la perte 24 h de la passerelle dans `/uptime`, `null` quand on ne sait pas', () => {
    const map = {
      '192.168.1.1': { uptime_pct: 99.1, samples: 10 },
      '1.1.1.1': { uptime_pct: 97, samples: 10 },
    };
    expect(gatewayLoss({ gateway: '192.168.1.1', monitors: [] }, map)).toBeCloseTo(0.9, 5);
    expect(gatewayLoss({ gateway: '10.0.0.1', monitors: [] }, map)).toBeCloseTo(0.9, 5);
    expect(gatewayLoss({ gateway: null, monitors: [] }, {})).toBeNull();
    expect(
      gatewayLoss({ gateway: 'x', monitors: [] }, { x: { uptime_pct: null, samples: 0 } }),
    ).toBeNull();
  });
});
