/**
 * L'état AFFICHÉ d'une sonde — la forme et le mot de la DA « Relevé ».
 *
 * La fraîcheur (`probe-liveness.ts`) reste la seule source sur le battement ;
 * ce module y ajoute ce que l'écran sait en plus :
 *
 * — `never` : enrôlée, jamais vue. La fraîcheur la range en « hors ligne »,
 *   parce qu'elle ne bat pas ; mais on ne l'a jamais vue en ligne, et le dire
 *   « hors ligne depuis — » enverrait chercher une panne qui n'existe pas.
 * — `degraded` : la sonde bat, mais son accès internet est limité ou coupé
 *   (`internet_state`). Battement et mesures ne passent pas par la même
 *   requête : un voyant vert seul cacherait exactement ce cas.
 *
 * ⚠️ « Dégradée » ne vaut que pour une sonde EN LIGNE. Sur une sonde muette,
 * l'état internet est la dernière valeur reçue : il n'apprend rien de plus que
 * le silence.
 */

import type { Probe } from './api';
import { probeLiveness } from './probe-liveness';

export type DisplayState = 'online' | 'degraded' | 'silent' | 'offline' | 'never';

export interface ProbeDisplay {
  state: DisplayState;
  /** Depuis quand, en secondes epoch ; `null` quand il n'y a rien à dater. */
  since: number | null;
  /** Ce que dit internet, pour l'état `degraded` seulement. */
  internet: 'limited' | 'offline' | null;
}

type Fields = Pick<Probe, 'last_seen' | 'created_at'> &
  Partial<Pick<Probe, 'internet_state' | 'internet_state_at'>>;

export function probeDisplay(probe: Fields, atMs: number): ProbeDisplay {
  if (probe.last_seen == null) {
    return { state: 'never', since: probe.created_at ?? null, internet: null };
  }
  const live = probeLiveness(probe.last_seen, atMs);
  if (live.state !== 'online') {
    return { state: live.state, since: probe.last_seen, internet: null };
  }
  const net = probe.internet_state;
  if (net === 'limited' || net === 'offline') {
    return { state: 'degraded', since: probe.internet_state_at ?? null, internet: net };
  }
  return { state: 'online', since: null, internet: null };
}

export type DisplayTotals = Record<DisplayState, number>;

/** Le relevé chiffré du parc, état par état. */
export function displayTotals(probes: Fields[], atMs: number): DisplayTotals {
  const t: DisplayTotals = { online: 0, degraded: 0, silent: 0, offline: 0, never: 0 };
  for (const p of probes) t[probeDisplay(p, atMs).state]++;
  return t;
}
