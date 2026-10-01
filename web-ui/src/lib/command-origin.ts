/**
 * D'où vient une commande (contrat § 26).
 *
 * 🔴 **Trois réponses, pas deux.** « Hub », « cet appareil-là », et
 * **« inconnue »** — cette dernière pour les commandes empilées avant le § 26,
 * dont le hub n'a jamais enregistré la provenance. Les ranger par défaut du
 * côté du hub mettrait une affirmation fausse dans la seule trace qui réponde
 * à « qui a lancé ce scan sur le réseau de Durand », des mois après. Une
 * colonne vide se lit ; une colonne qui ment ne se rattrape pas.
 *
 * ⚠️ Le nom rendu ici est celui que le hub avait **à l'empilement**, figé en
 * base. Ce module ne va jamais le rechercher dans la liste des appareils : un
 * téléphone qui change de main se fait renommer, et le nom d'aujourd'hui
 * attribuerait à son nouveau porteur un ordre lancé par l'ancien.
 */

/** Ce que la colonne « Origine » doit afficher pour une ligne. */
export interface CommandOriginView {
  kind: 'hub' | 'device' | 'unknown';
  /**
   * Le nom de l'appareil **tel qu'il était alors**, ou son identifiant à
   * défaut. `null` pour le hub, pour l'inconnu, et pour un appareil dont on ne
   * tient ni nom ni identifiant — l'écran dit alors « appareil » tout court,
   * ce qui reste vrai.
   */
  label: string | null;
}

/** Les seules valeurs que le hub écrit. Tout le reste est une inconnue. */
const KNOWN = ['hub', 'device'] as const;

/**
 * Ce qu'une ligne de commande dit de sa provenance.
 *
 * ⚠️ C'est `origin` qui tranche, jamais les colonnes voisines. Déduire
 * « appareil » de la présence d'un identifiant donnerait un second chemin de
 * décision, et deux chemins finissent par diverger — ici, en affichant comme
 * venant d'un téléphone une ligne que le hub a marquée « hub ».
 *
 * ⚠️ Une valeur que cette version ne connaît pas (un hub plus récent, un
 * onglet resté ouvert pendant une mise à jour) est rendue inconnue, pas
 * affichée brute : un mot anglais non traduit dans le tableau se lirait comme
 * un défaut, et la prendre pour « hub » mentirait.
 */
export function commandOrigin(row: {
  origin?: string | null;
  origin_device_id?: string | null;
  origin_device_name?: string | null;
}): CommandOriginView {
  const origin = row.origin?.trim();
  if (!origin || !(KNOWN as readonly string[]).includes(origin)) {
    return { kind: 'unknown', label: null };
  }
  if (origin === 'hub') {
    return { kind: 'hub', label: null };
  }
  // Le nom d'alors d'abord, l'identifiant ensuite. Retomber sur « inconnue »
  // parce qu'un nom manque effacerait ce qu'on sait : que l'ordre vient d'un
  // appareil appairé, et lequel.
  const name = row.origin_device_name?.trim();
  const id = row.origin_device_id?.trim();
  return { kind: 'device', label: name || id || null };
}
