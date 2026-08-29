/** Types et couleurs partagés par les graphiques du hub. */

export interface Serie {
  key: string;
  label: string;
  /** Variable CSS, jamais une valeur en dur : le thème clair a ses propres pas. */
  color: string;
  points: { t: number; v: number }[];
}

/**
 * Deux emplacements catégoriels, dans un ordre fixe : « descendant » garde
 * toujours la même couleur, même si la série « montant » est vide. Une couleur
 * attribuée au rang trompe le lecteur qui a mémorisé la première lecture.
 *
 * Valeurs et validation : voir `web-ui/src/web.css`.
 */
export const SERIES = {
  one: 'var(--lp-series-1)',
  two: 'var(--lp-series-2)',
} as const;

/** Couleur de la n-ième courbe d'un même graphique — cycle si on dépasse. */
export function seriesColor(index: number): string {
  const n = (index % 5) + 1;
  return `var(--lp-series-${n})`;
}

/** États internet écrits par la sonde (`crates/lanprobe-core/src/internet.rs`). */
export type InternetState = 'online' | 'limited' | 'offline' | 'unknown';

export function normalizeState(raw: unknown): InternetState {
  const s = String(raw ?? '').toLowerCase();
  if (s === 'online' || s === 'up') return 'online';
  if (s === 'limited' || s === 'degraded') return 'limited';
  if (s === 'offline' || s === 'down') return 'offline';
  return 'unknown';
}


/**
 * Découpe une courbe en segments continus, en coupant sur les interruptions.
 *
 * ⚠️ Sans ça, arrêter une surveillance puis la reprendre traçait une droite
 * parfaite au travers de l'interruption — une mesure affirmée pour une période
 * où personne ne mesurait, et d'autant plus trompeuse que la ligne est lisse.
 *
 * La limite se calcule sur la cadence observée, pas sur une constante : une
 * sonde qui pingue toutes les 2 s et une qui teste son débit toutes les 6 h
 * n'ont pas la même idée d'un trou. En dessous de trois points il n'y a pas de
 * cadence à observer — on ne coupe rien plutôt que de couper au hasard.
 */
export function gapLimit(points: { t: number }[]): number {
  if (points.length < 3) return Infinity;
  const deltas = points.slice(1).map((p, i) => p.t - points[i].t).sort((a, b) => a - b);
  const median = deltas[Math.floor(deltas.length / 2)] || 60_000;
  return Math.max(median * 4, 30_000);
}

export function segments<T extends { t: number }>(points: T[]): T[][] {
  const limit = gapLimit(points);
  const out: T[][] = [];
  let current: T[] = [];
  for (const [i, p] of points.entries()) {
    if (i > 0 && p.t - points[i - 1].t > limit) {
      out.push(current);
      current = [];
    }
    current.push(p);
  }
  if (current.length) out.push(current);
  return out;
}
