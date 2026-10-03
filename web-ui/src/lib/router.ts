import { readable } from 'svelte/store';

/**
 * Routage par hash, sans dépendance.
 *
 * Le hash évite d'imposer une réécriture d'URL au backend et au reverse proxy :
 * le hub sert un seul `index.html`, quelle que soit la profondeur. Le détail
 * d'une sonde reste malgré tout adressable — on peut coller `#/probes/0c1e…`
 * dans un ticket.
 */

/**
 * Onglets de Réglages. Ils sont dans l'URL et non dans un état local, pour la
 * même raison que la fiche d'une sonde : « ouvre #/settings/storage » se dicte
 * au téléphone, « clique sur Réglages puis sur le troisième onglet » non.
 *
 * Les identifiants restent en anglais et ne suivent pas la langue de
 * l'interface : un lien collé dans un ticket ne doit pas cesser de fonctionner
 * parce que son destinataire a mis le hub en français.
 */
export const SETTINGS_TABS = [
  // ⚠️ Pas de `account` ici : « Mon compte » n'est plus un onglet de Réglages
  // depuis `07d69a9`, il a son entrée dans la barre de gauche et son adresse
  // propre `#/account`. Le remettre dans cette liste ferait réapparaître un
  // onglet dans la bande, et surligner « Réglages » sur son écran.
  'general',
  'realtime',
  'alerts',
  'storage',
  // Les profils de scan de ports, partagés par tout le hub (contrat § 25).
  // Ils vivent dans les Réglages et non sur la fiche d'une sonde : ils ne sont
  // à aucune sonde en particulier, et les éditer depuis l'une d'elles ferait
  // croire le contraire.
  'portscan',
  'backups',
  'accounts',
] as const;
export type SettingsTab = (typeof SETTINGS_TABS)[number];

export type Route =
  | { name: 'fleet' }
  | { name: 'probe'; id: string }
  | { name: 'audit' }
  // 🔴 « Mon compte » est une route à part entière, et non `settings/account`.
  // Il a quitté la bande d'onglets pour la barre de gauche ; tant que son
  // adresse restait sous `settings/`, elle désignait un écran qu'elle
  // n'affichait plus — et tout ce qui lit la route devait le rattraper par un
  // cas particulier (c'est ce qui surlignait « Réglages » sur cet écran).
  | { name: 'account' }
  | { name: 'settings'; tab: SettingsTab };

/**
 * Adresses d'hier, redirigées plutôt que perdues.
 *
 * Elles ne sont pas supprimées : une URL qui retombe en silence sur le parc
 * laisse croire que l'écran a disparu. `settings/account` est le cas du
 * 02/10 — un onglet resté ouvert, ou rechargé après la mise à jour du hub,
 * doit arriver sur « Mon compte » et pas sur « Général ».
 *
 * La clé est un chemin, pas seulement un premier segment : c'est ce qui permet
 * de déplacer une adresse à deux niveaux sans toucher à celle du niveau au
 * dessus, qui elle existe toujours.
 */
const MOVED: Record<string, Route> = {
  accounts: { name: 'settings', tab: 'accounts' },
  notifications: { name: 'settings', tab: 'alerts' },
  'settings/account': { name: 'account' },
};

/** Le chemin d'un hash, découpé — sans le `#`, sans la requête, sans les vides. */
function segments(hash: string): string[] {
  return hash
    .replace(/^#\/?/, '')
    .split('?')[0]
    .split('/')
    .filter(Boolean);
}

/** La route d'arrivée si ce chemin est une adresse déplacée. */
function movedTo(parts: string[]): Route | undefined {
  return MOVED[parts.join('/')] ?? MOVED[parts[0]];
}

function isTab(v: string | undefined): v is SettingsTab {
  return SETTINGS_TABS.includes(v as SettingsTab);
}

export function parse(hash: string): Route {
  const parts = segments(hash);
  // Les adresses déplacées d'abord : `settings/account` doit être reconnue
  // avant la branche `settings`, qui la prendrait pour un onglet inconnu et la
  // ferait retomber sur « Général ».
  const moved = movedTo(parts);
  if (moved) return moved;
  if (parts[0] === 'probes' && parts[1]) return { name: 'probe', id: decodeURIComponent(parts[1]) };
  if (parts[0] === 'audit') return { name: 'audit' };
  if (parts[0] === 'account') return { name: 'account' };
  if (parts[0] === 'settings') return { name: 'settings', tab: isTab(parts[1]) ? parts[1] : 'general' };
  // `#/enroll` n'existe plus : l'enrôlement se fait sur la ligne du site, dans
  // le parc. Un ancien lien y retombe donc, plutôt que sur un écran vide.
  return { name: 'fleet' };
}

/** L'adresse canonique d'une route — celle qu'on veut voir dans la barre. */
export function href(r: Route): string {
  switch (r.name) {
    case 'probe':
      return `#/probes/${encodeURIComponent(r.id)}`;
    case 'audit':
      return '#/audit';
    case 'account':
      return '#/account';
    case 'settings':
      return `#/settings/${r.tab}`;
    default:
      return '#/';
  }
}

/**
 * Remet l'adresse déplacée sur sa forme actuelle, **sans** ajouter d'entrée
 * dans l'historique : `replaceState` plutôt qu'une écriture de `location.hash`,
 * pour que le bouton « précédent » ne renvoie pas sur l'ancienne adresse qui
 * redirigerait aussitôt — une page dont on ne peut pas sortir en arrière.
 */
function canonicalize(r: Route) {
  const want = href(r);
  // ⚠️ Seules les adresses DÉPLACÉES sont réécrites. Comparer `href(parse(…))`
  // au hash sans cette condition réécrirait aussi les adresses inconnues, qui
  // retombent sur le parc : `#/pigeon-voyageur` deviendrait `#/` et on ne
  // saurait plus, en relisant la barre, ce qui avait été demandé.
  if (location.hash !== want && movedTo(segments(location.hash))) {
    history.replaceState(null, '', want);
  }
}

export const route = readable<Route>(parse(location.hash), (set) => {
  const apply = () => {
    const r = parse(location.hash);
    canonicalize(r);
    set(r);
  };
  apply();
  addEventListener('hashchange', apply);
  return () => removeEventListener('hashchange', apply);
});

export function go(to: string) {
  location.hash = to.startsWith('#') ? to : `#${to}`;
}
