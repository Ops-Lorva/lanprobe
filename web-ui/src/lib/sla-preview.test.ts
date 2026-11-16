import { describe, expect, it } from 'vitest';
import { summaryLines, siteOutages } from './sla-report';
import type { Sample, SlaPayload } from './sla-report';

const t = (k: string) => k;
const s = (ts: number, alive: boolean, latency_ms = 10): Sample => ({ timestamp: ts, alive, latency_ms });

function payload(probe: string, gw: Sample[], internet: Sample[] = []): SlaPayload {
  return {
    probe,
    site: 'Atelier',
    range: '-7d',
    generated_at: 1_788_000_000,
    targets: [{ ip: '192.168.1.1', samples: gw }],
    internet,
    speedtests: [],
    discovery: null,
    ports: null,
    public_ip_history: [],
  };
}

describe('summaryLines', () => {
  it('donne une ligne par cible, internet d’abord, avec ses statistiques', () => {
    const lines = summaryLines(
      [payload('rdc', [s(1, true), s(2, false), s(3, true), s(4, true)], [{ timestamp: 1, alive: true, state: 'online' }])],
      t,
    );
    expect(lines.map((l) => [l.probe, l.label])).toEqual([
      ['rdc', 'sla.internet'],
      ['rdc', '192.168.1.1'],
    ]);
    expect(lines[1].stats.uptime_pct).toBe(75);
    expect(lines[1].outages).toHaveLength(1);
  });
});

describe('siteOutages', () => {
  it('rassemble les coupures de toutes les sondes, les plus récentes d’abord', () => {
    const lines = summaryLines(
      [
        payload('a', [s(1, false), s(2, true)]),
        payload('b', [s(5, true), s(6, false), s(7, false)]),
      ],
      t,
    );
    const out = siteOutages(lines);
    expect(out.map((o) => [o.probe, o.start, o.end])).toEqual([
      ['b', 6, null],
      ['a', 1, 2],
    ]);
  });
});
