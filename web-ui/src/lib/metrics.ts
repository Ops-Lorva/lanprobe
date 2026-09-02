/**
 * Lecture des mesures via `GET /api/probes/{id}/metrics`.
 *
 * ⚠️ Le contrat décrit l'endpoint (« requête Flux proxifiée ») mais PAS la forme
 * de la réponse. Plutôt que de parier sur une seule structure, on normalise les
 * formes plausibles d'un proxy Flux, et on échoue explicitement — message lisible
 * + clés observées — sur tout le reste. Un écran qui dit « format inattendu :
 * results, meta » se débogue ; un `undefined.map` non.
 */

export interface Point {
  /** Epoch en millisecondes. */
  t: number;
  v: number | string;
}

/** Séries indexées par nom de champ Influx (`latency_ms`, `state`, …). */
export type SeriesMap = Record<string, Point[]>;

export class MetricsShapeError extends Error {
  constructor(readonly observed: string) {
    super(`metrics_shape:${observed}`);
    this.name = 'MetricsShapeError';
  }
}

/** Mesures écrites par la sonde (cf. `event_to_points`, crates/lanprobe-server). */
export const MEASUREMENT = {
  ping: 'ping_latency',
  internet: 'internet_status',
  speedtest: 'speedtest',
} as const;

/**
 * Fenêtres proposées, en durée Flux signée (`range(start: -6h)`).
 *
 * `-10m` est là pour le mode temps réel, où la sonde bat à 5 s et pingue à
 * 1 s : à l'échelle d'une heure, l'incident qu'on est venu regarder tient
 * dans un pixel. Elle reste proposée hors temps réel — on regarde souvent
 * quelque chose se produire pendant qu'on a les mains dans le réseau.
 */
export const RANGES = ['-10m', '-1h', '-6h', '-24h', '-7d'] as const;
export type Range = (typeof RANGES)[number];
export const DEFAULT_RANGE: Range = '-6h';

/**
 * Durée d'une fenêtre en millisecondes (`-6h` → 21 600 000).
 *
 * ⚠️ La minute compte parmi les unités reconnues : le repli d'une valeur
 * illisible vaut une heure, et une fenêtre de 10 min bornerait alors son axe
 * sur 50 min de vide — ce qui se lit comme une sonde qui a cessé de mesurer.
 */
export function rangeMillis(range: Range): number {
  const m = /^-(\d+)([mhd])$/.exec(range);
  if (!m) return 3_600_000;
  const n = Number(m[1]);
  if (m[2] === 'd') return n * 86_400_000;
  return m[2] === 'm' ? n * 60_000 : n * 3_600_000;
}

/**
 * Ramène un horodatage à des millisecondes.
 * Influx date en nanosecondes, un backend peut renvoyer des secondes ou du
 * RFC3339 : on décide sur l'ordre de grandeur plutôt que de supposer.
 */
export function toMillis(raw: unknown): number | null {
  if (typeof raw === 'string') {
    const parsed = Date.parse(raw);
    if (!Number.isNaN(parsed)) return parsed;
    const asNum = Number(raw);
    if (Number.isFinite(asNum)) return toMillis(asNum);
    return null;
  }
  if (typeof raw !== 'number' || !Number.isFinite(raw)) return null;
  const abs = Math.abs(raw);
  if (abs > 1e17) return raw / 1e6; // nanosecondes
  if (abs > 1e14) return raw / 1e3; // microsecondes
  if (abs > 1e11) return raw; // millisecondes
  return raw * 1000; // secondes
}

const TIME_KEYS = ['_time', 'time', 'timestamp', 'ts', 't'];
const VALUE_KEYS = ['_value', 'value', 'v'];
const FIELD_KEYS = ['_field', 'field', 'name', 'series'];

function pick(row: Record<string, unknown>, keys: string[]): unknown {
  for (const k of keys) if (k in row && row[k] != null) return row[k];
  return undefined;
}

/** Lignes plates : `[{ _time, _field, _value }, …]` — sortie usuelle d'un proxy Flux. */
function fromRows(rows: Record<string, unknown>[], fallbackField: string): SeriesMap {
  const out: SeriesMap = {};
  for (const row of rows) {
    if (!row || typeof row !== 'object') continue;
    const t = toMillis(pick(row, TIME_KEYS));
    if (t == null) continue;

    const rawField = pick(row, FIELD_KEYS);
    if (typeof rawField === 'string') {
      const v = pick(row, VALUE_KEYS);
      if (typeof v === 'number' || typeof v === 'string') {
        (out[rawField] ??= []).push({ t, v });
      }
      continue;
    }

    // Ligne « pivotée » : un objet par instant, une clé par champ.
    let matched = false;
    for (const [k, v] of Object.entries(row)) {
      if (TIME_KEYS.includes(k) || k.startsWith('_') || k === 'host' || k === 'probe_id') continue;
      if (typeof v === 'number' || typeof v === 'string') {
        (out[k] ??= []).push({ t, v });
        matched = true;
      }
    }
    if (!matched) {
      const v = pick(row, VALUE_KEYS);
      if (typeof v === 'number' || typeof v === 'string') (out[fallbackField] ??= []).push({ t, v });
    }
  }
  return out;
}

/** Format colonnes/valeurs (Influx v1, ou un proxy qui garde la forme tabulaire). */
function fromColumns(columns: string[], values: unknown[][]): SeriesMap {
  const out: SeriesMap = {};
  const timeIdx = columns.findIndex((c) => TIME_KEYS.includes(c));
  if (timeIdx < 0) throw new MetricsShapeError(columns.join(','));
  for (const rowValues of values) {
    const t = toMillis(rowValues[timeIdx]);
    if (t == null) continue;
    columns.forEach((col, i) => {
      if (i === timeIdx) return;
      const v = rowValues[i];
      if (typeof v === 'number' || typeof v === 'string') (out[col] ??= []).push({ t, v });
    });
  }
  return out;
}

/**
 * Normalise la réponse de l'endpoint metrics, quelle que soit sa forme.
 * `fallbackField` nomme la série quand la réponse ne porte pas de nom de champ.
 */
export function normalizeMetrics(payload: unknown, fallbackField = 'value'): SeriesMap {
  if (payload == null) return {};
  if (Array.isArray(payload)) {
    if (payload.length === 0) return {};
    return sortSeries(fromRows(payload as Record<string, unknown>[], fallbackField));
  }
  if (typeof payload !== 'object') throw new MetricsShapeError(typeof payload);

  const obj = payload as Record<string, unknown>;

  // Enveloppes courantes : { data: … }, { results: … }, { series: … }.
  for (const key of ['data', 'rows', 'points', 'results']) {
    const inner = obj[key];
    if (Array.isArray(inner)) {
      if (inner.length && typeof inner[0] === 'object' && inner[0] !== null) {
        const first = inner[0] as Record<string, unknown>;
        if ('series' in first || 'columns' in first) return normalizeMetrics(first, fallbackField);
      }
      return normalizeMetrics(inner, fallbackField);
    }
    if (inner && typeof inner === 'object') return normalizeMetrics(inner, fallbackField);
  }

  if (Array.isArray(obj.columns) && Array.isArray(obj.values)) {
    return sortSeries(fromColumns(obj.columns as string[], obj.values as unknown[][]));
  }

  if (Array.isArray(obj.series)) {
    const out: SeriesMap = {};
    for (const entry of obj.series as Record<string, unknown>[]) {
      if (!entry || typeof entry !== 'object') continue;
      if (Array.isArray(entry.columns) && Array.isArray(entry.values)) {
        const part = fromColumns(entry.columns as string[], entry.values as unknown[][]);
        for (const [k, v] of Object.entries(part)) (out[k] ??= []).push(...v);
        continue;
      }
      const name =
        typeof pick(entry, FIELD_KEYS) === 'string' ? String(pick(entry, FIELD_KEYS)) : fallbackField;
      const pts = (entry.points ?? entry.data ?? entry.values) as unknown;
      if (!Array.isArray(pts)) continue;
      const bucket = (out[name] ??= []);
      for (const p of pts) {
        if (Array.isArray(p)) {
          const t = toMillis(p[0]);
          const v = p[1];
          if (t != null && (typeof v === 'number' || typeof v === 'string')) bucket.push({ t, v });
        } else if (p && typeof p === 'object') {
          const row = p as Record<string, unknown>;
          const t = toMillis(pick(row, TIME_KEYS));
          const v = pick(row, VALUE_KEYS);
          if (t != null && (typeof v === 'number' || typeof v === 'string')) bucket.push({ t, v });
        }
      }
    }
    return sortSeries(out);
  }

  throw new MetricsShapeError(Object.keys(obj).slice(0, 6).join(', ') || '{}');
}

/**
 * Une courbe à tracer : un champ Influx pour un jeu d'étiquettes donné.
 *
 * ⚠️ `field` est le nom NU du champ (`latency_ms`), `label` ce qu'on affiche
 * (`latency_ms · 10.0.30.12`). La distinction n'est pas cosmétique : le hub
 * scinde une mesure en autant de courbes qu'il y a de cibles surveillées, et
 * décore alors le libellé. Un écran qui cherchait ses points sous `latency_ms`
 * n'en trouvait plus aucun dès qu'une étiquette existait — c'est exactement
 * comme ça que les trois graphiques de l'onglet sonde sont restés vides alors
 * qu'Influx contenait des milliers de points.
 */
export interface Curve {
  field: string;
  label: string;
  /** Étiquettes Influx de la courbe (`engine`, `ip`, …). */
  tags: Record<string, string>;
  points: Point[];
}

/**
 * Normalise vers une liste de courbes, en préservant la séparation par
 * étiquettes. Deux cibles pinguées en parallèle donnent deux courbes ; les
 * fusionner entrelacerait leurs points et dessinerait une latence qui
 * n'existe nulle part.
 */
export function normalizeCurves(payload: unknown, fallbackField = 'value'): Curve[] {
  const entries =
    payload && typeof payload === 'object' && !Array.isArray(payload)
      ? (payload as Record<string, unknown>).series
      : undefined;

  if (Array.isArray(entries)) {
    const out: Curve[] = [];
    let understood = false;
    for (const raw of entries as Record<string, unknown>[]) {
      if (!raw || typeof raw !== 'object' || !Array.isArray(raw.points)) continue;
      const label = typeof raw.field === 'string' ? raw.field : fallbackField;
      const field = typeof raw.measure === 'string' ? raw.measure : label;
      const points: Point[] = [];
      for (const p of raw.points as unknown[]) {
        if (!p || typeof p !== 'object') continue;
        const row = p as Record<string, unknown>;
        const t = toMillis(pick(row, TIME_KEYS));
        const v = pick(row, VALUE_KEYS);
        if (t != null && (typeof v === 'number' || typeof v === 'string')) points.push({ t, v });
      }
      points.sort((a, b) => a.t - b.t);
      const tags =
        raw.tags && typeof raw.tags === 'object' && !Array.isArray(raw.tags)
          ? Object.fromEntries(
              Object.entries(raw.tags as Record<string, unknown>)
                .filter(([, v]) => typeof v === 'string')
                .map(([k, v]) => [k, v as string]),
            )
          : {};
      out.push({ field, label, tags, points });
      understood = true;
    }
    // Une enveloppe `series` d'une autre forme (colonnes/valeurs) retombe sur
    // le normaliseur générique plutôt que de rendre une liste vide.
    if (understood || entries.length === 0) return out;
  }

  return Object.entries(normalizeMetrics(payload, fallbackField)).map(([field, points]) => ({
    field,
    label: field,
    tags: {},
    points,
  }));
}

function sortSeries(map: SeriesMap): SeriesMap {
  for (const key of Object.keys(map)) map[key].sort((a, b) => a.t - b.t);
  return map;
}

/** Ne garde que les points numériques d'une série (les champs texte sortent). */
/**
 * Délai d'attente d'un ping, côté sonde (`lanprobe-core/src/ping.rs`).
 */
export const PING_TIMEOUT_MS = 1000;

/**
 * Écarte les latences que le réseau ne peut pas avoir produites.
 *
 * ⚠️ Une latence supérieure au délai d'attente est impossible : le ping aurait
 * abandonné avant. Elle vient d'un processus suspendu — un portable endormi
 * pendant la mesure reprend son chronomètre au réveil et compte tout le
 * sommeil comme temps de réponse. Relevé en production : 1 045 504 ms, soit
 * 17 minutes, qui poussaient l'axe du graphe à 1 500 000 et aplatissaient
 * toutes les mesures honnêtes.
 *
 * La sonde ne les écrit plus, mais celles déjà dans InfluxDB restent tant
 * qu'elles sont dans la fenêtre : sans ce filtre, les graphes resteraient
 * écrasés pendant toute la rétention.
 *
 * ⚠️ On retire la LATENCE, jamais la disponibilité. Réécrire après coup un
 * `alive` déjà enregistré changerait un pourcentage de SLA remis à un client —
 * ce n'est pas la même décision, et elle n'a pas été prise.
 */
export function plausibleLatency(
  points: { t: number; v: number }[],
): { t: number; v: number }[] {
  return points.filter((p) => p.v <= PING_TIMEOUT_MS);
}

export function numeric(points: Point[] | undefined): { t: number; v: number }[] {
  if (!points) return [];
  const out: { t: number; v: number }[] = [];
  for (const p of points) {
    const n = typeof p.v === 'number' ? p.v : Number(p.v);
    if (Number.isFinite(n)) out.push({ t: p.t, v: n });
  }
  return out;
}
