import { describe, expect, it } from 'vitest';
import {
  DEFAULT_RANGE,
  RANGES,
  aggregatedEvery,
  normalizeCurves,
  pairPeaks,
  normalizeMetrics,
  numeric,
  rangeMillis,
  toMillis,
} from './metrics';

/**
 * Réponse réelle du hub, relevée sur une sonde qui pingue deux cibles.
 * Le libellé porte l'étiquette, `measure` porte le nom nu du champ — c'est
 * exactement la paire que l'interface avait cessé de savoir lire.
 */
const HUB_PING = {
  range: '-6h',
  series: [
    {
      field: 'latency_ms · 10.0.30.12',
      measure: 'latency_ms',
      tags: { ip: '10.0.30.12' },
      points: [
        { t: 1_788_013_668_322, v: 0.4 },
        { t: 1_788_013_670_802, v: 0.3 },
      ],
    },
    {
      field: 'latency_ms · 8.8.8.8',
      measure: 'latency_ms',
      tags: { ip: '8.8.8.8' },
      points: [{ t: 1_788_013_668_400, v: 22.1 }],
    },
    {
      field: 'alive · 10.0.30.12',
      measure: 'alive',
      tags: { ip: '10.0.30.12' },
      points: [{ t: 1_788_013_668_322, v: 1 }],
    },
  ],
};

describe('normalizeCurves', () => {
  it('retrouve le champ sous son nom nu, malgré un libellé étiqueté', () => {
    // Le défaut du 29/08 : l'écran cherchait `latency_ms`, le hub envoyait
    // `latency_ms · 10.0.30.12`, et le graphique restait vide sans erreur.
    const curves = normalizeCurves(HUB_PING, 'latency_ms');
    const latency = curves.filter((c) => c.field === 'latency_ms');
    expect(latency).toHaveLength(2);
    expect(latency[0].label).toBe('latency_ms · 10.0.30.12');
  });

  it('garde une courbe par cible, sans les fusionner', () => {
    // Fusionnées, les deux cibles entrelaceraient leurs points et
    // dessineraient une latence qu'aucune machine n'a mesurée.
    const curves = normalizeCurves(HUB_PING, 'latency_ms');
    const targets = curves.filter((c) => c.field === 'latency_ms').map((c) => c.tags.ip);
    expect(new Set(targets).size).toBe(2);
  });

  it('expose les étiquettes, dont le moteur de speedtest', () => {
    // Sans `tags.engine`, impossible de tracer Ookla et iperf3 séparément :
    // un seul des deux moteurs finissait sur le graphique.
    const curves = normalizeCurves({
      series: [
        {
          field: 'download_mbps · ookla',
          measure: 'download_mbps',
          tags: { engine: 'ookla' },
          points: [{ t: 1_788_013_668_322, v: 901.6 }],
        },
        {
          field: 'download_mbps · iperf3',
          measure: 'download_mbps',
          tags: { engine: 'iperf3' },
          points: [{ t: 1_788_013_670_000, v: 940 }],
        },
      ],
    });
    expect(curves.map((c) => c.tags.engine)).toEqual(['ookla', 'iperf3']);
  });

  it('conserve les valeurs texte, que la bande d’état colorie', () => {
    // `state` n'a pas de place sur un axe, mais la bande d'état ne trace
    // pas : elle colorie des intervalles nommés. L'écarter la vidait.
    const curves = normalizeCurves({
      series: [
        {
          field: 'state',
          measure: 'state',
          tags: {},
          points: [
            { t: 1_788_013_668_322, v: 'online' },
            { t: 1_788_013_670_000, v: 'offline' },
          ],
        },
      ],
    });
    expect(curves[0].points.map((p) => p.v)).toEqual(['online', 'offline']);
  });

  it('trie les points par instant, quel que soit l’ordre reçu', () => {
    const curves = normalizeCurves({
      series: [
        {
          field: 'v',
          measure: 'v',
          tags: {},
          points: [{ t: 1_788_013_670_000, v: 1 }, { t: 1_788_013_668_322, v: 2 }],
        },
      ],
    });
    expect(curves[0].points.map((p) => p.t)).toEqual([1_788_013_668_322, 1_788_013_670_000]);
  });

  it('rend une liste vide plutôt que d’échouer sur une réponse vide', () => {
    expect(normalizeCurves({ range: '-1h', series: [] })).toEqual([]);
    expect(normalizeCurves(null)).toEqual([]);
  });

  it('retombe sur le normaliseur générique pour une forme inconnue', () => {
    // Un proxy Flux d'une autre version rend des lignes plates : on ne veut
    // pas d'un écran vide parce que la forme a changé.
    const curves = normalizeCurves([{ _time: 1_788_013_668_322, _field: 'latency_ms', _value: 7 }]);
    expect(curves).toHaveLength(1);
    expect(curves[0].field).toBe('latency_ms');
    expect(curves[0].tags).toEqual({});
  });
});

describe('normalizeMetrics', () => {
  it('accepte les lignes plates d’un proxy Flux', () => {
    const map = normalizeMetrics([
      { _time: '2026-08-29T09:00:00Z', _field: 'latency_ms', _value: 12.5 },
    ]);
    expect(map.latency_ms).toHaveLength(1);
  });

  it('accepte la forme colonnes/valeurs', () => {
    const map = normalizeMetrics({
      columns: ['_time', 'latency_ms'],
      values: [['2026-08-29T09:00:00Z', 12.5]],
    });
    expect(map.latency_ms[0].v).toBe(12.5);
  });
});

describe('rangeMillis', () => {
  it('traduit les fenêtres proposées par l’interface', () => {
    // C'est cette durée qui borne l'axe : fausse, un graphique « 6 h »
    // recommencerait à se caler sur l'étendue des points.
    expect(rangeMillis('-1h')).toBe(3_600_000);
    expect(rangeMillis('-6h')).toBe(21_600_000);
    expect(rangeMillis('-24h')).toBe(86_400_000);
    expect(rangeMillis('-7d')).toBe(604_800_000);
  });

  it('comprend la minute : sans elle, « 10 min » retombait sur 1 h', () => {
    // Le repli silencieux de `rangeMillis` vaut une heure. Une fenêtre de
    // 10 min bornerait donc son axe sur 60 min de vide, et le graphique se
    // lirait comme une sonde qui ne mesure plus.
    expect(rangeMillis('-10m')).toBe(600_000);
  });
});

describe('RANGES', () => {
  it('propose 10 min en plus court, sans déplacer la fenêtre par défaut', () => {
    // En temps réel la sonde bat à 5 s et pingue à 1 s : à l'échelle d'une
    // heure, ce qu'on est venu regarder tient dans un pixel.
    expect(RANGES[0]).toBe('-10m');
    expect(RANGES).toContain('-1h');
    // ⚠️ On ajoute un choix, on n'en retire pas — et le défaut ne bouge pas.
    expect(DEFAULT_RANGE).toBe('-6h');
  });
});

describe('toMillis', () => {
  it('décide sur l’ordre de grandeur, pas sur une supposition d’unité', () => {
    // Nanosecondes : au-delà de 2^53 la division perd les décimales de la
    // milliseconde. Sans conséquence pour un graphique, mais il vaut mieux
    // que le test le dise que de le découvrir sur une égalité stricte.
    expect(toMillis(1_788_013_668_322_000_000)).toBeCloseTo(1_788_013_668_322, 0);
    expect(toMillis(1_788_013_668_322)).toBe(1_788_013_668_322);
    expect(toMillis(1_788_013_668)).toBe(1_788_013_668_000);
    expect(toMillis('2026-08-29T09:00:00Z')).toBe(Date.parse('2026-08-29T09:00:00Z'));
    expect(toMillis('pas une date')).toBeNull();
  });
});

describe('numeric', () => {
  it('écarte le texte sans écarter le point suivant', () => {
    expect(numeric([{ t: 1, v: 'online' }, { t: 2, v: 3 }])).toEqual([{ t: 2, v: 3 }]);
    expect(numeric(undefined)).toEqual([]);
  });
});

/**
 * Réponse agrégée du hub : le pas est annoncé, et chaque champ arrive en
 * DEUX séries — la moyenne sous son nom nu, le maximum de l'intervalle sous
 * le suffixe `_max`.
 */
const HUB_AGGREGE = {
  range: '-87d',
  aggregated_every: '3h',
  series: [
    {
      field: 'latency_ms · 10.0.30.12',
      measure: 'latency_ms',
      tags: { ip: '10.0.30.12' },
      points: [
        { t: 1_788_000_000_000, v: 12 },
        { t: 1_788_010_800_000, v: 12.4 },
      ],
    },
    {
      field: 'latency_ms_max · 10.0.30.12',
      measure: 'latency_ms_max',
      tags: { ip: '10.0.30.12' },
      points: [
        { t: 1_788_000_000_000, v: 12 },
        { t: 1_788_010_800_000, v: 800 },
      ],
    },
    {
      field: 'latency_ms · 8.8.8.8',
      measure: 'latency_ms',
      tags: { ip: '8.8.8.8' },
      points: [{ t: 1_788_000_000_000, v: 22 }],
    },
    {
      field: 'latency_ms_max · 8.8.8.8',
      measure: 'latency_ms_max',
      tags: { ip: '8.8.8.8' },
      points: [{ t: 1_788_000_000_000, v: 24 }],
    },
  ],
};

describe('aggregatedEvery', () => {
  it('rend le pas annoncé, et null quand la réponse est brute', () => {
    // ⚠️ Le champ est ABSENT sur une réponse brute, jamais à « 1s ». Le lire
    // comme un pas ferait annoncer une moyenne là où il n'y en a aucune.
    expect(aggregatedEvery(HUB_AGGREGE)).toBe('3h');
    expect(aggregatedEvery(HUB_PING)).toBeNull();
    expect(aggregatedEvery(null)).toBeNull();
    expect(aggregatedEvery([])).toBeNull();
  });
});

describe('pairPeaks', () => {
  it('rattache chaque maximum à SA moyenne, jamais à celle d’à côté', () => {
    // ⚠️ L'appariement se fait sur les étiquettes autant que sur le champ :
    // deux cibles pinguées en parallèle ont chacune leur pic, et croiser les
    // deux dessinerait un maximum qu'aucune machine n'a mesuré.
    const paired = pairPeaks(normalizeCurves(HUB_AGGREGE, 'latency_ms'));
    expect(paired).toHaveLength(2);

    const local = paired.find((c) => c.tags.ip === '10.0.30.12')!;
    expect(local.field).toBe('latency_ms');
    // La moyenne de l'intervalle reste à 12,4 ms : parfaitement crédible,
    // alors qu'un pic à 800 ms s'y est produit. C'est le maximum qui le garde.
    expect(local.points.at(-1)!.v).toBe(12.4);
    expect(local.peak?.at(-1)!.v).toBe(800);

    const google = paired.find((c) => c.tags.ip === '8.8.8.8')!;
    expect(google.peak?.[0].v).toBe(24);
  });

  it('laisse les courbes brutes intactes, sans maximum', () => {
    const paired = pairPeaks(normalizeCurves(HUB_PING, 'latency_ms'));
    expect(paired.every((c) => c.peak === undefined)).toBe(true);
    expect(paired).toHaveLength(normalizeCurves(HUB_PING, 'latency_ms').length);
  });

  it('garde un maximum orphelin plutôt que de le faire disparaître', () => {
    // Ne devrait pas arriver — les deux séries sortent du même flux — mais
    // escamoter une série reçue est le genre de silence que ce dépôt refuse.
    const orphan = normalizeCurves(
      {
        series: [
          {
            field: 'latency_ms_max',
            measure: 'latency_ms_max',
            tags: {},
            points: [{ t: 1, v: 9 }],
          },
        ],
      },
      'latency_ms',
    );
    expect(pairPeaks(orphan)).toHaveLength(1);
  });
});
