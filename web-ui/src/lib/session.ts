import { derived, writable } from 'svelte/store';
import type { Identity, Role } from './api';
import { applyServerLang } from './i18n';

/**
 * Qui est connecté, et avec quel rôle.
 *
 * Le hub le dit lui-même : `GET /api/me` rend `{ username, role }` derrière le
 * garde de session. Avant elle, l'interface apprenait son rôle **en se prenant
 * un refus** — or le contrat § 11 impose qu'un refus écrive une ligne
 * `access.denied` au journal. Chaque ouverture de session d'un observateur y
 * déposait donc un refus que personne n'avait provoqué : exactement le bruit
 * qui empêche de repérer, dans ce même journal, la série de refus qui compte.
 *
 * **Rien n'est mis en cache.** La valeur est relue au démarrage et à chaque
 * connexion. Un rôle gardé en `sessionStorage` survivrait à une rétrogradation
 * et mentirait tant que l'onglet reste ouvert — alors que le hub, lui, relit le
 * rôle à chaque requête.
 *
 * ⚠️ **Ce n'est pas une protection.** Elle est côté serveur, route par route.
 * Ici on masque seulement ce qui n'aboutirait pas : un onglet qu'un rôle ne
 * peut pas utiliser ne s'affiche pas pour lui, et c'est tout ce que ça fait.
 */
export const identity = writable<Identity | null>(null);

/** `null` seulement avant la première réponse de `/api/me`. */
export const role = derived(identity, ($i): Role | null => $i?.role ?? null);

export const isAdmin = derived(identity, ($i) => $i?.role === 'admin');

/**
 * `operator` **ou** `admin` : les rôles sont ordonnés (`viewer < operator <
 * admin`), une route ouverte à `operator` l'est donc aussi à `admin`.
 */
export const canOperate = derived(
  identity,
  ($i) => $i?.role === 'admin' || $i?.role === 'operator',
);

/**
 * ⚠️ **C'est ici que la langue du compte s'applique**, et pas dans une des
 * vues : `setIdentity` est le seul passage obligé des deux chemins d'entrée —
 * le démarrage et la connexion. Le faire dans l'un des deux seulement laissait
 * l'autre en anglais, et le défaut ne se serait vu qu'à l'usage.
 */
export function setIdentity(who: Identity) {
  identity.set(who);
  applyServerLang(who.lang);
}

/** À la déconnexion : le compte suivant n'hérite de rien. */
export function forgetIdentity() {
  identity.set(null);
}
