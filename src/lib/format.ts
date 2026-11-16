/** Mise en forme partagée par les écrans : chiffres, statistiques d'une cible, tracés. */

const cache = new Map<string, Intl.NumberFormat>();

/** Un nombre dans la langue de l'interface ; un tiret quand il n'y a rien. */
export function fmtNum(v: number | null | undefined, lang: string, digits = 1): string {
  if (v == null || !Number.isFinite(v)) return '—';
  const key = `${lang}:${digits}`;
  let f = cache.get(key);
  if (!f) {
    f = new Intl.NumberFormat(lang, { minimumFractionDigits: digits, maximumFractionDigits: digits });
    cache.set(key, f);
  }
  return f.format(v);
}

interface Sample { alive: boolean; latency_ms: number | null }

export interface TargetStats {
  min: number | null;
  avg: number | null;
  max: number | null;
  p95: number | null;
  /** Part des relevés sans réponse, en %. */
  loss: number | null;
  /** Part des relevés avec réponse, en %. */
  uptime: number | null;
  total: number;
  failed: number;
}

export function targetStats(history: Sample[]): TargetStats {
  const total = history.length;
  const failed = history.filter((s) => !s.alive).length;
  const lats = history
    .filter((s) => s.alive && s.latency_ms != null)
    .map((s) => s.latency_ms as number)
    .sort((a, b) => a - b);
  const n = lats.length;
  return {
    min: n ? lats[0] : null,
    max: n ? lats[n - 1] : null,
    avg: n ? lats.reduce((a, b) => a + b, 0) / n : null,
    p95: n ? lats[Math.min(n - 1, Math.ceil(n * 0.95) - 1)] : null,
    loss: total ? (failed / total) * 100 : null,
    uptime: total ? ((total - failed) / total) * 100 : null,
    total,
    failed,
  };
}

/**
 * Les segments d'une courbe de latence, en coordonnées SVG.
 * ⚠️ Un trou où la cible ne répondait pas n'est pas relié : une ligne droite
 * par-dessus masquerait la panne.
 */
export function sparkPoints(history: Sample[], w: number, h: number, pad = 2): [number, number][][] {
  if (history.length === 0) return [];
  const vals = history.map((p) => (p.alive ? p.latency_ms : null));
  const max = Math.max(1, ...vals.filter((v): v is number => v != null));
  const denom = Math.max(1, history.length - 1);
  const segs: [number, number][][] = [];
  let cur: [number, number][] | null = null;
  vals.forEach((v, i) => {
    if (v == null) { cur = null; return; }
    if (!cur) { cur = []; segs.push(cur); }
    cur.push([+((i / denom) * w).toFixed(1), +(h - pad - (v / max) * (h - 2 * pad)).toFixed(1)]);
  });
  return segs;
}

/** `255.255.255.0` → 24 ; `null` si le masque n'est pas contigu ou illisible. */
export function maskToPrefix(mask: string): number | null {
  const parts = mask.split('.').map(Number);
  if (parts.length !== 4 || parts.some((p) => !Number.isInteger(p) || p < 0 || p > 255)) return null;
  const bits = parts.map((p) => p.toString(2).padStart(8, '0')).join('');
  return /^1*0*$/.test(bits) ? bits.indexOf('0') === -1 ? 32 : bits.indexOf('0') : null;
}
