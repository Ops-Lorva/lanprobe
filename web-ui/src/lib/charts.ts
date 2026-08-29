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
