/**
 * Filtres, compteurs et tris du parc — tous fondés sur `probeLiveness`.
 *
 * ⚠️ **Le défaut que ce module corrige était visible à l'écran.** Les lignes
 * lisaient déjà la fraîcheur calculée dans le navigateur, pendant que la barre
 * de filtres, le bandeau du parc et les compteurs de site lisaient encore
 * `p.status`. Le hub tolère trois battements manqués : une sonde affichée
 * « Silencieuse » sur sa ligne était comptée « En ligne » dans le bandeau, et
 * retenue par le filtre « En ligne ». Deux affirmations contradictoires sur le
 * même écran, dont une fausse — et c'est celle qui rassure qu'on croit.
 *
 * Les règles vivent ici, dans un `.ts` avec ses tests : le projet ne monte pas
 * de composants, une règle écrite dans un gabarit ne serait couverte par rien.
 * C'est exactement comme ça que les deux implémentations ont divergé.
 */

import type { Probe } from './api';
import { probeLiveness, type LivenessState } from './probe-liveness';

/**
 * Ce que la barre de filtres peut demander.
 *
 * `buffering` n'est pas un état de fraîcheur : une sonde parfaitement en ligne
 * peut accumuler des points qu'elle n'arrive pas à écrire. Le filtre coexiste
 * donc avec les trois autres au lieu de s'y substituer.
 */
export type FleetFilter = LivenessState | 'all' | 'buffering';

/**
 * Ordre de lecture : ce qui appelle une action d'abord. Sur trente lignes,
 * l'œil balaie une colonne — le rouge doit toujours occuper la même position.
 */
export const LIVENESS_ORDER = ['offline', 'silent', 'online'] as const;

/** Rang d'un état pour le tri. Plus petit = plus urgent. */
const RANK: Record<LivenessState, number> = { offline: 0, silent: 1, online: 2 };

export interface FleetTotals extends Record<LivenessState, number> {
  buffered: number;
}

/** Fraîcheur d'une sonde à un instant donné. Un seul point d'entrée. */
export function livenessOf(probe: Probe, atMs: number): LivenessState {
  return probeLiveness(probe.last_seen, atMs).state;
}

export function matchesFleetFilter(probe: Probe, filter: FleetFilter, atMs: number): boolean {
  if (filter === 'all') return true;
  if (filter === 'buffering') return probe.buffered_points > 0;
  return livenessOf(probe, atMs) === filter;
}

/** Compteurs du bandeau du parc. Les points en tampon s'additionnent à part. */
export function fleetTotals(probes: Probe[], atMs: number): FleetTotals {
  const t: FleetTotals = { online: 0, silent: 0, offline: 0, buffered: 0 };
  for (const p of probes) {
    t[livenessOf(p, atMs)]++;
    t.buffered += p.buffered_points || 0;
  }
  return t;
}

/** Compteurs par état d'un site, tels que son en-tête les affiche. */
export function livenessCounts(probes: Probe[], atMs: number): Record<LivenessState, number> {
  const c: Record<LivenessState, number> = { online: 0, silent: 0, offline: 0 };
  for (const p of probes) c[livenessOf(p, atMs)]++;
  return c;
}

/**
 * Gravité d'un site, pour trier les sections. Plus petit = remonte.
 *
 * ⚠️ Un site VIDE vaut la même chose qu'un site sain, pas mieux : il n'a rien
 * à signaler, mais il n'a rien prouvé non plus. Le faire passer devant un site
 * en panne enfouirait la panne sous des sites sans sonde.
 */
export function siteSeverity(probes: Probe[], atMs: number): number {
  const c = livenessCounts(probes, atMs);
  if (c.offline > 0) return 0;
  if (c.silent > 0) return 1;
  if (probes.some((p) => p.buffered_points > 0)) return 2;
  return 3;
}

/** Clé de tri d'une ligne dans son site : hors ligne d'abord, en ligne ensuite. */
export function sortKey(probe: Probe, atMs: number): number {
  return RANK[livenessOf(probe, atMs)];
}

/**
 * Un site mérite-t-il d'être déplié d'office ?
 *
 * ⚠️ `!== 'online'` et non `=== 'offline'` : une sonde silencieuse est
 * précisément celle qu'on veut voir sans avoir à déplier, puisque l'en-tête
 * replié ne dit pas depuis quand elle se tait.
 */
export function needsAttention(probes: Probe[], atMs: number): boolean {
  return probes.some((p) => livenessOf(p, atMs) !== 'online' || p.buffered_points > 0);
}
