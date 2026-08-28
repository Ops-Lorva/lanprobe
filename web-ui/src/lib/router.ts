import { readable } from 'svelte/store';

/**
 * Routage par hash, sans dépendance.
 *
 * Le hash évite d'imposer une réécriture d'URL au backend et au reverse proxy :
 * le hub sert un seul `index.html`, quelle que soit la profondeur. Le détail
 * d'une sonde reste malgré tout adressable — on peut coller `#/probes/0c1e…`
 * dans un ticket.
 */
export type Route =
  | { name: 'fleet' }
  | { name: 'probe'; id: string }
  | { name: 'enroll' }
  | { name: 'settings' };

export function parse(hash: string): Route {
  const path = hash.replace(/^#\/?/, '').split('?')[0];
  const parts = path.split('/').filter(Boolean);
  if (parts[0] === 'probes' && parts[1]) return { name: 'probe', id: decodeURIComponent(parts[1]) };
  if (parts[0] === 'enroll') return { name: 'enroll' };
  if (parts[0] === 'settings') return { name: 'settings' };
  return { name: 'fleet' };
}

export const route = readable<Route>(parse(location.hash), (set) => {
  const onChange = () => set(parse(location.hash));
  addEventListener('hashchange', onChange);
  return () => removeEventListener('hashchange', onChange);
});

export function go(to: string) {
  location.hash = to.startsWith('#') ? to : `#${to}`;
}
