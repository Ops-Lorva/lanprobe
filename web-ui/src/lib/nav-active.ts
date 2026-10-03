/**
 * Quelle entrée de la barre latérale est surlignée, pour une route donnée.
 *
 * 🔴 **La règle est un simple appariement de nom, et c'est le but.** Elle ne
 * l'était pas : l'écran « Mon compte » avait quitté les onglets de Réglages
 * pour la barre de gauche en gardant l'adresse `#/settings/account`, si bien
 * que la route disait « réglages » là où l'écran disait « mon compte ». Le
 * gabarit rattrapait ça par un test sur l'onglet, et « Réglages » restait
 * surligné. L'adresse est devenue `#/account` (02/10) et le rattrapage a
 * disparu avec elle.
 *
 * ⚠️ Donc : si un `if` par route réapparaît ici, ce n'est pas le surlignage
 * qui est à corriger, c'est la route — elle ne désigne pas l'écran qu'elle
 * affiche.
 *
 * ⚠️ Et cette règle vit dans un `.ts` plutôt que dans un `class:active={…}` du
 * gabarit : écrite dans le gabarit, elle n'était couverte par rien, et c'est
 * exactement là que le défaut a vécu.
 */

import type { Route } from './router';

/** L'identifiant d'une entrée de la barre — `items` de `Shell.svelte`. */
export type NavId = 'fleet' | 'audit' | 'settings' | 'account';

const NAV_IDS: NavId[] = ['fleet', 'audit', 'settings', 'account'];

/**
 * L'entrée à surligner, ou `null` quand aucune ne correspond.
 *
 * `null` est le cas de la fiche d'une sonde : elle n'a pas d'entrée dans la
 * barre, et surligner « Parc » dirait qu'on est sur la liste alors qu'on est
 * sur une fiche.
 */
export function activeNavId(route: Route): NavId | null {
  return NAV_IDS.find((id) => id === route.name) ?? null;
}
