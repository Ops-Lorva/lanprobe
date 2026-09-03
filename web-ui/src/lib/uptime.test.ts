import { describe, expect, it } from 'vitest';
import { normalizeUptime, unmeasuredLabel, uptimeOf } from './uptime';

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

describe('unmeasuredLabel', () => {
  /** Fenêtre de 6 h en secondes, celle de la capture de Benjamin. */
  const FROM = 1_788_000_000;
  const TO = FROM + 6 * 3_600 - 1;

  /** Relevés à la seconde, du dernier quart d'heure de la fenêtre seulement. */
  const REVEIL_TARDIF = {
    uptime_pct: 99.5,
    samples: 900,
    first_at: TO - 899,
    last_at: TO,
  };

  it('dit la part de la fenêtre dont on est SÛR qu’elle n’a aucun relevé', () => {
    // 🔴 Le défaut : « 99,5 % » se lisait « tout va bien depuis 6 h » alors
    // qu'il disait « tout allait bien pendant le quart d'heure mesuré ».
    const label = unmeasuredLabel(REVEIL_TARDIF, FROM, TO, (k, v) => `${k}:${v?.pct}`);
    expect(label).toBe('probe.uptime_gap:95.8');
  });

  it('ne dit rien quand les relevés couvrent la fenêtre', () => {
    // ⚠️ Un « 0 % » permanent est du bruit qui masquera le cas utile — même
    // règle que `coverageLabel` du rapport.
    const plein = { uptime_pct: 99.5, samples: 21_600, first_at: FROM, last_at: TO };
    expect(unmeasuredLabel(plein, FROM, TO)).toBeNull();
  });

  it('tolère le décalage d’un relevé sans le prendre pour un trou', () => {
    // Le seuil est l'intervalle MOYEN OBSERVÉ × 3, jamais une cadence
    // supposée : à 1 s de cadence, 2 s de retard au démarrage n'est rien.
    const decale = { uptime_pct: 100, samples: 21_598, first_at: FROM + 2, last_at: TO };
    expect(unmeasuredLabel(decale, FROM, TO)).toBeNull();
  });

  it('ne dit rien d’un hub qui n’envoie pas les bornes', () => {
    // ⚠️ Absence d'information n'est pas absence de trou : on n'invente pas
    // « complet » sur le silence d'un hub antérieur à cette route.
    expect(unmeasuredLabel({ uptime_pct: 99.5, samples: 600 }, FROM, TO)).toBeNull();
  });

  it('ne dit rien sous deux relevés, faute d’intervalle observable', () => {
    // ⚠️ Un relevé isolé ne donne aucun intervalle, donc aucun seuil. On
    // n'en suppose pas un — c'est le refus tenu par `compute_coverage`.
    const seul = { uptime_pct: 100, samples: 1, first_at: TO, last_at: TO };
    expect(unmeasuredLabel(seul, FROM, TO)).toBeNull();
  });

  it('ne dit rien d’une cible inconnue ni d’une fenêtre vide', () => {
    expect(unmeasuredLabel(null, FROM, TO)).toBeNull();
    expect(unmeasuredLabel(REVEIL_TARDIF, TO, FROM)).toBeNull();
  });

  it('reste EN DEÇÀ du trou que le rapport calculera sur les mêmes relevés', () => {
    // 🔴 La règle qui rend les deux écrans compatibles : la carte ne compte
    // que les bords, le rapport y ajoute les trous intérieurs. La carte peut
    // donc en dire MOINS que le rapport, jamais plus — deux chiffres qui se
    // contredisent seraient pires que pas de chiffre du tout.
    const troue = { uptime_pct: 99.5, samples: 600, first_at: FROM, last_at: TO };
    expect(unmeasuredLabel(troue, FROM, TO)).toBeNull();
  });
});
