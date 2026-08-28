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

/** Fenêtres proposées, en durée Flux signée (`range(start: -6h)`). */
export const RANGES = ['-1h', '-6h', '-24h', '-7d'] as const;
export type Range = (typeof RANGES)[number];
export const DEFAULT_RANGE: Range = '-6h';

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

function sortSeries(map: SeriesMap): SeriesMap {
  for (const key of Object.keys(map)) map[key].sort((a, b) => a.t - b.t);
  return map;
}

/** Ne garde que les points numériques d'une série (les champs texte sortent). */
export function numeric(points: Point[] | undefined): { t: number; v: number }[] {
  if (!points) return [];
  const out: { t: number; v: number }[] = [];
  for (const p of points) {
    const n = typeof p.v === 'number' ? p.v : Number(p.v);
    if (Number.isFinite(n)) out.push({ t: p.t, v: n });
  }
  return out;
}
