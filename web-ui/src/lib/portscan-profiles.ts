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

/**
 * Les profils **de base** du hub, et eux seuls, restent traduits.
 *
 * ⚠️ `infra` n'y est plus semé depuis le 02/10 — l'application sonde n'en a
 * pas — mais il reste traduit : les hubs déjà en service le portent, et on ne
 * supprime rien ici. Lui retirer son libellé l'afficherait « Infra » en
 * français à quelqu'un qui le lisait traduit.
 */
const SEEDED_LABELS: Record<string, string> = {
  common: 'probe.profile_common',
  web: 'probe.profile_web',
  infra: 'probe.profile_infra',
  db: 'probe.profile_db',
  remote: 'probe.profile_remote',
  full: 'probe.profile_full',
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
 * Le profil avec lequel une machine a été scannée, tel qu'on l'affiche.
 *
 * 🔴 **L'identifiant est le fait, le nom n'est qu'une commodité.** Le hub le
 * résout à la lecture depuis sa table des profils (`latest_scan`) ; il n'est
 * jamais rangé avec le scan, sans quoi il serait figé à l'instant du scan et
 * divergerait au premier renommage.
 *
 * 🔴 **Profil supprimé depuis : l'identifiant SEUL.** On ne rend pas le nom de
 * la pierre tombale — l'unicité du nom ne vaut que parmi les profils vivants,
 * donc un profil neuf peut l'avoir repris avec une autre liste de ports
 * derrière, et le technicien irait lire la mauvaise liste. On n'invente jamais
 * un nom.
 *
 * 🔴 **Aucun profil : on le dit.** Un scan lancé avant la v30 du schéma, ou
 * depuis la fenêtre de la sonde sans en choisir, n'en avait pas. Afficher un
 * profil par défaut dirait d'un scan qu'il a eu un réglage que personne ne lui
 * a donné.
 */
export function scanProfileLabel(
  host: { profile_id?: string | null; profile_name?: string | null },
  t: (key: string) => string,
): string {
  const id = host.profile_id?.trim();
  if (!id) return t('probe.profile_none');
  // ⚠️ L'absence de nom se teste AVANT la traduction d'un profil de base :
  // c'est elle qui dit « supprimé », et un profil de base se supprime comme
  // les autres.
  if (!host.profile_name?.trim()) return id;
  // Les profils de base restent traduits, comme dans l'écran des profils :
  // sans quoi le même profil s'appellerait « Courants » ici et « Common » là.
  const key = SEEDED_LABELS[id];
  return key ? t(key) : host.profile_name;
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
 * a la sienne. Même règle pour `udp_ports`.
 *
 * 🔴 **`profile_id` part avec la commande**, et c'est le correctif du 02/10 :
 * un scan lancé depuis le hub ou depuis le téléphone s'affichait SANS aucun
 * profil sur l'écran de la sonde, alors qu'un scan lancé dans sa fenêtre
 * montrait le sien. La sonde range l'étiquette avec le résultat ; elle ne la
 * valide pas — les ports font foi, et un scan qui marche ne doit pas tomber
 * parce que le profil ne lui est pas encore descendu.
 */
export function portScanArgs(
  ip: string,
  profile: PortscanProfile | undefined,
): { ip: string; ports?: number[]; udp_ports?: number[]; profile_id?: string } {
  const args: { ip: string; ports?: number[]; udp_ports?: number[]; profile_id?: string } = { ip };
  const ports = profile?.ports ?? [];
  if (ports.length > 0) args.ports = ports;
  if (profile) args.profile_id = profile.profile_id;
  const udp = profile?.udp_ports ?? [];
  if (udp.length > 0) args.udp_ports = udp;
  return args;
}
