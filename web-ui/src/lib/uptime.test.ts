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

/**
 * Le trou que `compute_coverage` calculera sur les MÊMES relevés, recopié de
 * `crates/lanprobe-core/src/sla.rs`.
 *
 * ⚠️ Il n'est pas là pour tester le rapport — il est là pour que la propriété
 * qui rend les deux écrans compatibles soit VÉRIFIÉE et pas seulement affirmée
 * en commentaire : ce que la carte annonce ne doit jamais dépasser ça.
 */
function trouDuRapport(times: number[], from: number, to: number): number {
  const windowSecs = Math.max(0, to - from + 1);
  if (windowSecs === 0) return 0;
  if (times.length < 2) return windowSecs;
  const t = [...times].sort((a, b) => a - b);
  const intervals = t.slice(1).map((v, i) => v - t[i]).sort((a, b) => a - b);
  const median = Math.max(1, intervals[Math.floor(intervals.length / 2)]);
  const seuil = median * 3;
  let gap = intervals.filter((d) => d > seuil).reduce((s, d) => s + (d - median), 0);
  const lead = t[0] - from;
  if (lead > seuil) gap += lead;
  const tail = to - t[t.length - 1];
  if (tail > seuil) gap += tail;
  return Math.min(Math.max(gap, 0), windowSecs);
}

/** Ce que le hub renvoie pour une cible, déduit de ses relevés. */
function commeLeHub(times: number[]) {
  const t = [...times].sort((a, b) => a - b);
  return {
    uptime_pct: 100,
    samples: t.length,
    first_at: t[0],
    last_at: t[t.length - 1],
    max_gap_secs: t.slice(1).reduce((m, v, i) => Math.max(m, v - t[i]), 0),
  };
}

/** Relevés à la seconde sur `[debut, debut + duree)`. */
function aLaSeconde(debut: number, duree: number): number[] {
  return Array.from({ length: duree }, (_, i) => debut + i);
}

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

  it('dit la part de la fenêtre dont on est SÛR qu\u2019elle n\u2019a aucun relevé', () => {
    // 🔴 Le défaut : « 99,5 % » se lisait « tout va bien depuis 6 h » alors
    // qu'il disait « tout allait bien pendant le quart d'heure mesuré ».
    const label = unmeasuredLabel(REVEIL_TARDIF, FROM, TO, (k, v) => `${k}:${v?.pct}`);
    expect(label).toBe('probe.uptime_gap:95.8');
  });

  it('voit le trou du MILIEU, relevés touchant les deux bords', () => {
    // 🔴 Le cas de Benjamin, et celui que les seules bornes laissaient passer :
    // la sonde a mesuré au début et à la fin, la carte se taisait, et 92 % de
    // la période n'avait aucun relevé.
    const dormeur = {
      uptime_pct: 99.5,
      samples: 695,
      first_at: FROM,
      last_at: TO,
      max_gap_secs: 20_000,
    };
    expect(unmeasuredLabel(dormeur, FROM, TO, (k, v) => `${k}:${v?.pct}`)).toBe(
      'probe.uptime_gap:92.3',
    );
  });

  it('ne dit rien quand les relevés couvrent la fenêtre', () => {
    // ⚠️ Un « 0 % » permanent est du bruit qui masquera le cas utile — même
    // règle que `coverageLabel` du rapport.
    const plein = {
      uptime_pct: 99.5,
      samples: 21_600,
      first_at: FROM,
      last_at: TO,
      max_gap_secs: 1,
    };
    expect(unmeasuredLabel(plein, FROM, TO)).toBeNull();
  });

  it('tolère le décalage d\u2019un relevé sans le prendre pour un trou', () => {
    // Le seuil sort de l'intervalle MOYEN OBSERVÉ, jamais d'une cadence
    // supposée : à 1 s de cadence, 2 s de retard au démarrage n'est rien.
    const decale = {
      uptime_pct: 100,
      samples: 21_598,
      first_at: FROM + 2,
      last_at: TO,
      max_gap_secs: 1,
    };
    expect(unmeasuredLabel(decale, FROM, TO)).toBeNull();
  });

  it('ne dit rien d\u2019un hub qui n\u2019envoie pas les bornes', () => {
    // ⚠️ Absence d'information n'est pas absence de trou : on n'invente pas
    // « complet » sur le silence d'un hub antérieur à cette route.
    expect(unmeasuredLabel({ uptime_pct: 99.5, samples: 600 }, FROM, TO)).toBeNull();
  });

  it('ne dit rien sous deux relevés, faute d\u2019intervalle observable', () => {
    // ⚠️ Un relevé isolé ne donne aucun intervalle, donc aucun seuil. On
    // n'en suppose pas un — c'est le refus tenu par `compute_coverage`.
    const seul = { uptime_pct: 100, samples: 1, first_at: TO, last_at: TO };
    expect(unmeasuredLabel(seul, FROM, TO)).toBeNull();
  });

  it('ne dit rien d\u2019une cible inconnue ni d\u2019une fenêtre vide', () => {
    expect(unmeasuredLabel(null, FROM, TO)).toBeNull();
    expect(unmeasuredLabel(REVEIL_TARDIF, TO, FROM)).toBeNull();
  });

  it('ne déborde pas le rapport quand la moyenne passe SOUS la médiane', () => {
    // 🔴 Le contre-exemple qui a cassé la première version. Trois intervalles
    // de 1 s puis quatre de 10 : la médiane vaut 10, la moyenne 6,1. Un seuil
    // à 3 × la MOYENNE est alors PLUS PERMISSIF que celui du rapport, et la
    // carte annonçait 36 % de trou là où le rapport n'en voit aucun. C'est le
    // facteur 2 du majorant `médiane ≤ 2 × moyenne` qui l'interdit.
    const times = [25, 26, 27, 28, 38, 48, 58, 68];
    expect(trouDuRapport(times, 0, 68)).toBe(0);
    expect(unmeasuredLabel(commeLeHub(times), 0, 68)).toBeNull();
  });

  it('reste EN DEÇÀ du trou du rapport sur toutes ces formes de relevés', () => {
    // 🔴 La propriété qui rend les deux écrans compatibles, vérifiée et pas
    // seulement affirmée : la carte compte les deux bords et le PLUS GRAND
    // trou, le rapport y ajoute tous les autres. Elle peut donc en dire moins,
    // jamais plus. Deux chiffres qui se contredisent seraient pires que pas de
    // chiffre du tout.
    const formes: [string, number[]][] = [
      ['sonde endormie au milieu', [...aLaSeconde(FROM, 300), ...aLaSeconde(FROM + 20_300, 1_300)]],
      ['réveil tardif', aLaSeconde(TO - 899, 900)],
      ['arrêt précoce', aLaSeconde(FROM, 900)],
      ['cadence régulière de 20 s', Array.from({ length: 1_080 }, (_, i) => FROM + i * 20)],
      ['couverture pleine', aLaSeconde(FROM, 21_600)],
      [
        'quantité de petits trous',
        Array.from({ length: 60 }, (_, b) => aLaSeconde(FROM + b * 360, 120)).flat(),
      ],
      ['deux relevés aux extrémités', [FROM, TO]],
    ];

    for (const [nom, times] of formes) {
      const rapport = trouDuRapport(times, FROM, TO);
      const label = unmeasuredLabel(commeLeHub(times), FROM, TO, (_k, v) => String(v?.pct));
      const carte = label === null ? 0 : (Number(label) / 100) * (TO - FROM + 1);
      // La tolérance est l'arrondi du dixième de point que l'écran affiche.
      expect(carte).toBeLessThanOrEqual(rapport + (TO - FROM + 1) / 1_000);
    }
  });
});
