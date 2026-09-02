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
  'account',
  'general',
  'realtime',
  'alerts',
  'storage',
  'accounts',
] as const;
export type SettingsTab = (typeof SETTINGS_TABS)[number];

export type Route =
  | { name: 'fleet' }
  | { name: 'probe'; id: string }
  | { name: 'audit' }
  | { name: 'settings'; tab: SettingsTab };

/**
 * Anciennes entrées de navigation, devenues des onglets de Réglages.
 *
 * Elles ne sont pas supprimées mais redirigées : ces adresses sont dans des
 * favoris, des tickets et des captures d'écran, et une URL qui retombe en
 * silence sur le parc laisse croire que l'écran a disparu.
 */
const MOVED: Record<string, SettingsTab> = {
  accounts: 'accounts',
  notifications: 'alerts',
};

function isTab(v: string | undefined): v is SettingsTab {
  return SETTINGS_TABS.includes(v as SettingsTab);
}

export function parse(hash: string): Route {
  const path = hash.replace(/^#\/?/, '').split('?')[0];
  const parts = path.split('/').filter(Boolean);
  if (parts[0] === 'probes' && parts[1]) return { name: 'probe', id: decodeURIComponent(parts[1]) };
  if (parts[0] === 'audit') return { name: 'audit' };
  if (parts[0] === 'settings') return { name: 'settings', tab: isTab(parts[1]) ? parts[1] : 'general' };
  const moved = MOVED[parts[0]];
  if (moved) return { name: 'settings', tab: moved };
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
  if (location.hash !== want && MOVED[location.hash.replace(/^#\/?/, '').split('/')[0]]) {
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
