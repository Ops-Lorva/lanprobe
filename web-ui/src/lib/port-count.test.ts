import { describe, expect, it } from 'vitest';
import type { ScanPort } from './api';
import { countPortFamilies, portCountWording } from './port-count';

const port = (proto: string, n = 80): ScanPort => ({ ip: '10.0.8.1', port: n, proto });

/**
 * Le `t` des écrans : `(k, v) => $_(k, { values: v })`. Ici il rend la clé et
 * son `n` pour que les tests parlent de la RÈGLE, pas du français.
 */
const t = (key: string, values?: Record<string, string | number>) =>
  values && 'n' in values ? `${key}(${values.n})` : key;

describe('countPortFamilies', () => {
  it('compte chaque famille, et le total', () => {
    expect(countPortFamilies([port('tcp'), port('tcp', 443), port('udp', 53)])).toEqual({
      total: 3,
      tcp: 2,
      udp: 1,
    });
  });

  it('ne se laisse pas tromper par la casse du protocole', () => {
    expect(countPortFamilies([port('TCP'), port('Udp', 53)])).toEqual({ total: 2, tcp: 1, udp: 1 });
  });

  it('compte dans le TOTAL un protocole qu’on ne sait pas nommer', () => {
    // Le port est ouvert : c'est le fait. On ne lui invente pas d'étiquette,
    // et on ne le tait pas non plus — d'où un total qui dépasse tcp + udp.
    expect(countPortFamilies([port('tcp'), port('sctp', 9899)])).toEqual({
      total: 2,
      tcp: 1,
      udp: 0,
    });
  });

  it('accepte une liste vide', () => {
    expect(countPortFamilies([])).toEqual({ total: 0, tcp: 0, udp: 0 });
  });
});

describe('portCountWording', () => {
  it('dit les deux familles quand les deux sont là', () => {
    expect(portCountWording({ total: 8, tcp: 4, udp: 4 }, t)).toBe(
      'probe.ports_count(8) · 4 TCP · 4 UDP',
    );
  });

  it('ne répète PAS une famille seule — « 2 ports · 2 TCP » dit deux fois la même chose', () => {
    expect(portCountWording({ total: 2, tcp: 2, udp: 0 }, t)).toBe('probe.ports_count_tcp(2)');
    expect(portCountWording({ total: 2, tcp: 0, udp: 2 }, t)).toBe('probe.ports_count_udp(2)');
  });

  it('garde le total quand un protocole sans nom s’y ajoute', () => {
    // 3 ports dont 2 TCP : le troisième compte sans étiquette inventée.
    expect(portCountWording({ total: 3, tcp: 2, udp: 0 }, t)).toBe('probe.ports_count(3) · 2 TCP');
  });

  it('n’écrit « TCP » ni « UDP » pour une famille absente', () => {
    expect(portCountWording({ total: 2, tcp: 0, udp: 0 }, t)).toBe('probe.ports_count(2)');
  });

  it('dit « 0 port ouvert » sans famille, plutôt que « 0 port TCP »', () => {
    // ⚠️ Sans cette garde, `tcp === total` serait vrai pour deux zéros et
    // l'écran attribuerait au TCP une absence de port.
    expect(portCountWording({ total: 0, tcp: 0, udp: 0 }, t)).toBe('probe.ports_count(0)');
  });
});
