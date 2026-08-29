import { describe, expect, it } from 'vitest';
import { logTime, absoluteTime, dateOnly } from './time';

describe('logTime', () => {
  it('garde la seconde : c’est par elle qu’on recoupe avec les logs', () => {
    // 2026-08-29T14:51:36Z. On n'assère pas la chaîne exacte — elle dépend de
    // l'ICU de la machine — mais la présence des secondes, qui est le point.
    const out = logTime(1_788_015_096, 'fr');
    expect(out).toMatch(/\d{2}:\d{2}:\d{2}/);
    expect(out).toMatch(/\d{2}\/\d{2}\/\d{4}/);
  });

  it('rend un tiret plutôt que « Invalid Date » sur une absence', () => {
    expect(logTime(null, 'fr')).toBe('—');
    expect(logTime(undefined, 'en')).toBe('—');
  });

  it('reste plus court que le format long, qui débordait la colonne', () => {
    expect(logTime(1_788_015_096, 'fr').length).toBeLessThan(
      absoluteTime(1_788_015_096, 'fr').length,
    );
  });
});

describe('dateOnly', () => {
  it('omet l’heure, sans se tromper de jour', () => {
    expect(dateOnly(1_788_015_096, 'fr')).not.toMatch(/\d{2}:\d{2}/);
    expect(dateOnly(null, 'fr')).toBe('—');
  });
});
