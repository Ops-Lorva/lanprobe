import { writable } from 'svelte/store';

/**
 * Ce que la session a le droit de faire, tel qu'on l'a APPRIS.
 *
 * ⚠️ Le hub n'expose aucune route qui dise « qui suis-je, et avec quel rôle ».
 * `GET /api/status` rend `needs_setup` et la version, `POST /api/login` rend
 * `{ ok: true }` : la session n'apporte donc pas son rôle avec elle.
 *
 * Sonder une route d'administration au démarrage donnerait la réponse, mais le
 * contrat § 11 impose qu'un refus écrive une ligne `access.denied` au journal.
 * Chaque ouverture de session d'un observateur y déposerait donc un refus que
 * personne n'a provoqué — exactement le bruit qui empêche de repérer, dans ce
 * même journal, la série de refus qui compte.
 *
 * D'où ce modèle : on ne devine rien, on **retient ce que le hub a répondu**.
 * Les entrées d'administration sont visibles tant qu'on ne sait pas ; le
 * premier 403 les retire. Le refus qui en résulte a alors été provoqué par un
 * clic réel, et sa trace au journal est légitime.
 *
 * **Ce n'est pas une protection** : elle est côté serveur, route par route, et
 * elle existe déjà. Ceci n'est que du confort de navigation.
 *
 * `null` = pas encore su.
 */
const KEY = 'lanprobe.hub.admin';

function remembered(): boolean | null {
  try {
    const v = sessionStorage.getItem(KEY);
    return v === 'yes' ? true : v === 'no' ? false : null;
  } catch {
    return null;
  }
}

export const isAdmin = writable<boolean | null>(remembered());

/**
 * Enregistre le verdict du hub. `sessionStorage` et non `localStorage` : une
 * promotion de rôle prend effet à la requête suivante côté serveur, un cache
 * qui survivrait à la fermeture du navigateur mentirait plus longtemps.
 */
export function noteAdmin(value: boolean) {
  isAdmin.set(value);
  try {
    sessionStorage.setItem(KEY, value ? 'yes' : 'no');
  } catch {
    /* navigation privée verrouillée : on garde la valeur en mémoire */
  }
}

/** À la déconnexion : le compte suivant n'a aucune raison d'hériter du verdict. */
export function forgetAdmin() {
  isAdmin.set(null);
  try {
    sessionStorage.removeItem(KEY);
  } catch {
    /* non bloquant */
  }
}
