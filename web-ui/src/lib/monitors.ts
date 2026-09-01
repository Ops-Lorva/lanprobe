/**
 * Liste des surveillances telle que l'écran doit la lire : l'union de ce que
 * le hub tient (`probe_monitors`) et de ce que la sonde annonce (`monitors`).
 *
 * ⚠️ Deux sources coexistent pour la même notion, volontairement et pour un
 * temps : la colonne historique `probes.monitors`, réécrite à chaque battement
 * et seule à porter les mesures (répond, latence, disponibilité), et la table
 * `probe_monitors`, qui porte l'appartenance et les suppressions datées. C'est
 * la seconde qui fait autorité sur QUI est surveillé, la première sur COMMENT
 * la cible se porte. Les confondre redonnerait à l'absence la valeur d'un
 * retrait — le défaut que ce chantier corrige.
 *
 * Ce découpage vit ici, et pas dans le `.svelte` : le projet ne monte pas de
 * composants, une règle écrite dans un gabarit ne serait couverte par rien.
 */

import type { MonitorEntry, MonitorState } from './api';

/**
 * Cible surveillée mais dont aucune mesure n'est encore annoncée.
 *
 * ⚠️ `samples` à 0 n'est pas un remplissage : c'est lui qui dit à l'écran de
 * montrer l'attente. Une disponibilité de 0 % et un « ne répond pas » inventés
 * se liraient comme une panne, sur une cible que la sonde n'a simplement pas
 * encore pinguée.
 */
function awaiting(target: string): MonitorState {
  return {
    ip: target,
    alive: false,
    latency_ms: null,
    min_ms: null,
    avg_ms: null,
    max_ms: null,
    uptime_pct: 0,
    samples: 0,
  };
}

/**
 * Surveillances actives, mesures comprises.
 *
 * `entries` à `null` = hub antérieur à la liste partagée : on rend l'annonce
 * telle quelle, comportement d'avant.
 *
 * ⚠️ Une cible annoncée par la sonde et INCONNUE du hub reste active. Une
 * sonde antérieure au partage n'envoie aucun changement : sa table est vide
 * alors qu'elle pingue depuis des mois. Seule une suppression datée sort une
 * cible de la liste.
 *
 * Rend `null` quand aucune des deux sources ne sait rien : « je ne sais pas »
 * et « aucune cible » ne se disent pas de la même façon à l'écran.
 */
export function effectiveMonitors(
  entries: MonitorEntry[] | null | undefined,
  announced: MonitorState[] | null | undefined,
): MonitorState[] | null {
  if (entries == null) return announced ?? null;
  if (entries.length === 0 && announced == null) return null;

  const stats = new Map((announced ?? []).map((m) => [m.ip, m]));
  const removed = new Set(entries.filter((e) => e.removed_at != null).map((e) => e.target));
  const placed = new Set<string>();
  const list: MonitorState[] = [];

  for (const e of entries) {
    if (e.removed_at != null) continue;
    placed.add(e.target);
    list.push(stats.get(e.target) ?? awaiting(e.target));
  }
  for (const m of announced ?? []) {
    // Le retrait est un fait daté ; l'annonce est un état qui n'a pas encore
    // reçu l'ordre — le hub ne joint jamais la sonde, il attend son battement.
    // C'est donc le fait qui gagne, sans quoi la grille et le sous-onglet
    // « Retirées » se contrediraient au même instant.
    if (removed.has(m.ip) || placed.has(m.ip)) continue;
    list.push(m);
  }
  return list;
}

/**
 * Cibles retirées, la suppression la plus récente en tête.
 *
 * On vient à ce sous-onglet pour rattraper ce qu'on a retiré à l'instant, pas
 * pour lire une archive.
 */
export function removedMonitors(entries: MonitorEntry[] | null | undefined): MonitorEntry[] {
  return (entries ?? [])
    .filter((e) => e.removed_at != null)
    .sort((a, b) => (b.removed_at ?? 0) - (a.removed_at ?? 0));
}
