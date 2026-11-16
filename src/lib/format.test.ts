import { describe, expect, it } from 'vitest';
import { fmtNum, maskToPrefix, sparkPoints, targetStats } from './format';

describe('fmtNum', () => {
  it('suit la langue : virgule en français, point en anglais', () => {
    expect(fmtNum(1.25, 'fr', 1)).toBe('1,3');
    expect(fmtNum(1.25, 'en', 1)).toBe('1.3');
  });
  it('rend un tiret quand il n’y a rien', () => {
    expect(fmtNum(null, 'fr')).toBe('—');
  });
});

describe('targetStats', () => {
  const s = (alive: boolean, ms: number | null) => ({ alive, latency_ms: ms, timestamp: 0 });
  it('calcule min, moyenne, max, perte et disponibilité', () => {
    const st = targetStats([s(true, 1), s(true, 3), s(false, null), s(true, 2)]);
    expect(st).toMatchObject({ min: 1, max: 3, avg: 2, loss: 25, uptime: 75, total: 4, failed: 1 });
  });
  it('sans mesure : rien d’inventé', () => {
    expect(targetStats([])).toMatchObject({ min: null, avg: null, loss: null, uptime: null });
  });
});

describe('sparkPoints', () => {
  it('coupe le tracé là où la cible ne répond pas', () => {
    const p = sparkPoints([{ alive: true, latency_ms: 1 }, { alive: false, latency_ms: null }, { alive: true, latency_ms: 2 }], 100, 20);
    expect(p).toHaveLength(2);
    expect(p[0]).toHaveLength(1);
  });
  it('rien à tracer sans mesure', () => {
    expect(sparkPoints([], 100, 20)).toEqual([]);
  });
});

describe('maskToPrefix', () => {
  it('convertit un masque en longueur de préfixe', () => {
    expect(maskToPrefix('255.255.255.0')).toBe(24);
    expect(maskToPrefix('255.255.240.0')).toBe(20);
  });
  it('refuse un masque non contigu ou illisible', () => {
    expect(maskToPrefix('255.0.255.0')).toBeNull();
    expect(maskToPrefix('')).toBeNull();
  });
});
