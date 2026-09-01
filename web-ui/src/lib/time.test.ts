import { describe, expect, it } from 'vitest';
import { logTime, absoluteTime, dateOnly, secondsLeft, countdown } from './time';

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

describe('secondsLeft', () => {
  // Le mode temps réel s'éteint tout seul : l'échéance est le seul état gardé,
  // et une échéance passée doit se lire comme « terminé », pas comme un
  // compte à rebours négatif qu'un écran afficherait tel quel.
  it('rend 0 sur une échéance absente, nulle ou déjà passée', () => {
    const at = 1_788_015_096_000;
    expect(secondsLeft(null, at)).toBe(0);
    expect(secondsLeft(undefined, at)).toBe(0);
    expect(secondsLeft(1_788_015_000, at)).toBe(0);
  });

  it('compte les secondes qui restent', () => {
    const at = 1_788_015_096_000;
    expect(secondsLeft(1_788_015_096 + 1800, at)).toBe(1800);
  });
});

describe('countdown', () => {
  it('passe en minutes au-delà d’une minute', () => {
    const out = countdown(1680, 'fr');
    expect(out).toMatch(/28/);
    expect(out).toMatch(/min/);
  });

  it('reste en secondes en dessous d’une minute', () => {
    const out = countdown(45, 'fr');
    expect(out).toMatch(/45/);
    expect(out).not.toMatch(/min/);
  });
});
