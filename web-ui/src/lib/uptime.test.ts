import { describe, expect, it } from 'vitest';
import { normalizeUptime, uptimeOf } from './uptime';

/** Réponse réelle de `GET /api/probes/{id}/uptime`. */
const PAYLOAD = {
  range: '-24h',
  targets: [
    { ip: '10.0.30.12', uptime_pct: 99.86111111, samples: 86_280 },
    { ip: '8.8.8.8', uptime_pct: null, samples: 0 },
  ],
};

describe('normalizeUptime', () => {
  it('indexe par cible et garde le compte de relevés', () => {
    const map = normalizeUptime(PAYLOAD);
    expect(map['10.0.30.12'].samples).toBe(86_280);
    expect(map['10.0.30.12'].uptime_pct).toBeCloseTo(99.86, 2);
  });

  it('garde `null` sans le transformer en zéro', () => {
    // 🔴 C'est le point qui compte : « 0 % » se lit comme une panne totale.
    // Un `?? 0` quelque part sur ce chemin annulerait toute la journée.
    const map = normalizeUptime(PAYLOAD);
    expect(map['8.8.8.8'].uptime_pct).toBeNull();
    expect(map['8.8.8.8'].samples).toBe(0);
  });

  it('ne s’effondre pas sur une réponse d’une autre forme', () => {
    expect(normalizeUptime(null)).toEqual({});
    expect(normalizeUptime({})).toEqual({});
    expect(normalizeUptime({ targets: 'nope' })).toEqual({});
  });
});

describe('uptimeOf', () => {
  it('rend le taux formaté quand il existe', () => {
    expect(uptimeOf(normalizeUptime(PAYLOAD), '10.0.30.12')).toEqual({
      pct: 99.86111111,
      samples: 86_280,
    });
  });

  it('rend null pour une cible sans relevé déterminé', () => {
    // L'écran affichera « indéterminé », pas « 0 % » ni une case vide.
    expect(uptimeOf(normalizeUptime(PAYLOAD), '8.8.8.8')).toEqual({ pct: null, samples: 0 });
  });

  it('rend null pour une cible que le hub ne connaît pas', () => {
    // ⚠️ Absence d'information ≠ absence de disponibilité. Une cible qu'on
    // vient d'ajouter n'a pas encore de ligne : on n'affiche rien.
    expect(uptimeOf(normalizeUptime(PAYLOAD), '1.1.1.1')).toBeNull();
  });
});
