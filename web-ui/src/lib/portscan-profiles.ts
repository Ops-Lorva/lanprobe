/**
 * Profils de scan de ports partagés, côté interface du hub (contrat § 25).
 *
 * Ces règles vivent ici et pas dans le `.svelte` : le projet ne monte pas de
 * composants, une règle écrite dans un gabarit ne serait couverte par rien.
 *
 * 🔴 **La règle à ne pas rater** : un profil sans ports veut dire « la sonde
 * garde sa liste », et ça ne se dit qu'en **n'envoyant pas** le champ `ports`.
 * Envoyer `[]` ferait un scan COMPLET là où on croyait restreindre — la sonde
 * traite une liste vide comme une absence de restriction.
 */

import type { PortscanProfile } from './api';

/** Les quatre profils posés par la migration, et eux seuls, restent traduits. */
const SEEDED_LABELS: Record<string, string> = {
  common: 'probe.profile_common',
  web: 'probe.profile_web',
  infra: 'probe.profile_infra',
  db: 'probe.profile_db',
};

/**
 * Le libellé d'un profil à l'écran.
 *
 * ⚠️ Les quatre profils d'origine étaient traduits quand ils vivaient dans le
 * code : les passer en base ne doit pas rendre le hub anglais pour qui l'avait
 * en français. Tous les autres portent le nom que quelqu'un leur a donné — le
 * traduire serait lui en inventer un autre.
 */
export function profileLabel(profile: PortscanProfile, t: (key: string) => string): string {
  const key = SEEDED_LABELS[profile.profile_id];
  return key ? t(key) : profile.name;
}

/**
 * Lit une liste de ports saisie à la main.
 *
 * Triée et dédoublonnée — le hub le refera de son côté, mais l'écran doit
 * annoncer le même nombre que ce qui sera enregistré. Ce qui n'est pas un port
 * est jeté plutôt qu'envoyé : une commande refusée par la sonde arriverait
 * longtemps après le clic, sans dire pourquoi.
 */
export function parsePorts(raw: string): number[] {
  const seen = new Set<number>();
  // ⚠️ On découpe sur les séparateurs **qu'on tape vraiment** — virgule,
  // espace, point-virgule, retour à la ligne — et non sur « tout ce qui n'est
  // pas un chiffre » : ce dernier ferait de `-1` un port 1, et de `http2` un
  // port 2. Un morceau qui n'est pas entièrement un nombre est jeté.
  for (const piece of raw.split(/[\s,;]+/)) {
    if (!/^\d+$/.test(piece)) continue;
    const port = Number(piece);
    if (port >= 1 && port <= 65535) seen.add(port);
  }
  return [...seen].sort((a, b) => a - b);
}

/** La forme qu'on relit — et qu'on recolle dans le champ de saisie. */
export function formatPorts(ports: number[]): string {
  return ports.join(', ');
}

/**
 * Les arguments de la commande `port_scan`.
 *
 * 🔴 **`ports` est OMIS quand le profil n'en porte pas.** Voir l'en-tête : un
 * tableau vide ferait un scan complet. C'est aussi pourquoi un profil absent
 * — liste vide côté hub, tous supprimés — lance quand même un scan : la sonde
 * a la sienne.
 */
export function portScanArgs(
  ip: string,
  profile: PortscanProfile | undefined,
): { ip: string; ports?: number[] } {
  const ports = profile?.ports ?? [];
  return ports.length > 0 ? { ip, ports } : { ip };
}
