import { describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

// Lecture du hub suspendue à la main : on observe l'état PENDANT la lecture.
const waiting: Array<() => void> = [];
const release = () => waiting.splice(0).forEach((r) => r());
vi.mock('./api', async (orig) => {
  const real = await orig<typeof import('./api')>();
  const pending = () =>
    new Promise<never[]>((resolve) => {
      waiting.push(() => resolve([]));
    });
  return {
    ...real,
    api: { ...real.api, sites: vi.fn(() => pending()), probes: vi.fn(() => pending()) },
  };
});

const { fleet } = await import('./fleet');

describe('relecture du parc', () => {
  it("une relecture de fond ne grise pas l'écran", async () => {
    const run = fleet.load(() => {}, { quiet: true });
    expect(get(fleet).refreshing).toBe(false);
    release();
    await run;
  });
});
