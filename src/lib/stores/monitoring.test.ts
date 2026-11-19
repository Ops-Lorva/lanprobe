import { describe, expect, it } from 'vitest';
import { withSample, type HostMonitor } from './monitoring';

const empty = (): HostMonitor => ({ ip: '10.0.0.1', current: null, history: [] });
const s = (timestamp_ms: number, alive: boolean) => ({
  timestamp: Math.floor(timestamp_ms / 1000),
  timestamp_ms,
  alive,
  latency_ms: alive ? 3 : null,
});

describe('withSample', () => {
  // 🔴 Un relevé muet (~2,5 s) finit après celui lancé une seconde plus tard.
  // Pris comme « dernier arrivé », il repeignait la ligne en rouge juste après
  // le vert : l'état clignotait pendant une panne partielle.
  it('ne laisse pas un relevé parti plus tôt écraser l’état affiché', () => {
    let h = withSample(empty(), s(10_000, true));
    h = withSample(h, s(12_000, true));
    h = withSample(h, s(11_000, false));
    expect(h.current).toEqual({ alive: true, latency_ms: 3 });
    expect(h.history.map((e) => e.timestamp_ms)).toEqual([10_000, 11_000, 12_000]);
  });

  it('range un relevé sans milliseconde à sa seconde', () => {
    let h = withSample(empty(), { timestamp: 12, alive: true, latency_ms: 3 });
    h = withSample(h, { timestamp: 11, alive: false, latency_ms: null });
    expect(h.current?.alive).toBe(true);
  });

  it('garde les 60 relevés les plus récents', () => {
    let h = empty();
    for (let i = 0; i < 70; i++) h = withSample(h, s(i * 1000, true));
    expect(h.history).toHaveLength(60);
    expect(h.history[0].timestamp_ms).toBe(10_000);
  });
});
