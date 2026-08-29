import { describe, expect, it } from 'vitest';
import { normalizeCurves, normalizeMetrics, numeric, rangeMillis, toMillis } from './metrics';

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
