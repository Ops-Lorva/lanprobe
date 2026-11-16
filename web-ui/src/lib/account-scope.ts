/**
 * La portée d'un compte (contrat § 17), telle que l'écran des comptes la lit
 * et l'envoie.
 *
 * 🔴 **« Tous les sites » est un choix explicite depuis la v31 du hub** (audit
 * RSSI du 16/11, F1). Une liste vide voulait dire « tout » : supprimer le
 * dernier site d'un compte restreint lui ouvrait alors tout le parc. Elle veut
 * désormais dire « rien », et « tout » se dit avec `all_sites: true`.
 *
 * ⚠️ Ce module ne traduit pas : il rend des clés du catalogue et leurs valeurs.
 */

import type { Role } from './api';

/** Ce qu'il faut savoir d'un compte pour dire ce qu'il voit. */
export interface ScopedAccount {
  role: Role;
  sites: string[];
  /** Absent sur un hub d'avant la v31 : la liste vide y valait « tout ». */
  all_sites?: boolean;
}

export type ScopeMode = 'all' | 'sites';

export interface ScopeSummary {
  key: string;
  values?: Record<string, number>;
}

/** Ce que voit un compte, en une phrase, pour la colonne du tableau. */
export function scopeSummary(a: ScopedAccount): ScopeSummary {
  if (a.role === 'admin') return { key: 'accounts.scope_admin' };
  if (isAll(a)) return { key: 'accounts.scope_all' };
  if (a.sites.length === 0) return { key: 'accounts.scope_nothing' };
  return { key: 'accounts.scope_n', values: { n: a.sites.length } };
}

/** L'état du dialogue à son ouverture : celui du compte, pas un défaut. */
export function initialScope(a: ScopedAccount): { mode: ScopeMode; picked: string[] } {
  return { mode: isAll(a) ? 'all' : 'sites', picked: [...a.sites] };
}

/**
 * Le corps du `PATCH`. ⚠️ Toujours explicite : `{ all_sites: true }` ou
 * `{ all_sites: false, sites }`. Une liste vide envoyée seule restreint à
 * rien — c'est voulu, et le dialogue le dit.
 */
export function scopePatch(
  mode: ScopeMode,
  picked: string[],
): { all_sites: true } | { all_sites: false; sites: string[] } {
  return mode === 'all' ? { all_sites: true } : { all_sites: false, sites: [...picked] };
}

function isAll(a: ScopedAccount): boolean {
  // Hub d'avant la v31 : pas de champ, et la liste vide y valait « tout ».
  return a.all_sites ?? a.sites.length === 0;
}
