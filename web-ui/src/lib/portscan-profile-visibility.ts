/**
 * Ce que le sélecteur de profils de scan de la SONDE affiche (contrat § 25).
 *
 * 🔴 **Une sonde rattachée à un hub n'affiche plus ses profils de base.**
 * Décision de Benjamin, 02/10 : « dès qu'une sonde passe en mode hub, elle
 * n'affiche plus du tout les profils de base qu'elle a en local — juste ceux
 * qu'elle a créés, et ceux venant du hub ».
 *
 * Ce qu'on corrige : l'écran montrait côte à côte ses profils de base et ceux
 * du hub, trois paires portant le même nom pour des listes DIFFÉRENTES —
 * `Common` 16 TCP/6 UDP en local contre 0/0 au hub, `Web` 13 contre 8,
 * `Databases` 13/1 contre 8/0. Masquer sur le nom aurait caché des ports
 * réellement scannés ; ne plus afficher les profils de base n'en cache aucun,
 * puisque le hub sème désormais les mêmes et fait autorité.
 *
 * ⚠️ **C'est une décision d'AFFICHAGE, pas une suppression.** Les profils de
 * base restent dans l'application et resservent tels quels le jour où la sonde
 * est désenrôlée — c'est le seul moment où ils servent encore, et c'est aussi
 * ce qui fait que l'écran de scan marche sans hub.
 *
 * ⚠️ Les règles vivent ici et pas dans le `.svelte` : une règle écrite dans un
 * gabarit n'est couverte par rien.
 *
 * ⚠️ **Et elles vivent dans `web-ui/`, bien que seule l'application de bureau
 * les consomme** (`src/lib/components/PortScan.svelte`, `Settings.svelte`).
 * C'est le seul endroit du dépôt où vitest tourne. Posées dans `src/lib/`,
 * elles n'auraient aucun test : le harnais de `web-ui` sait importer un
 * `.svelte` du bureau par l'alias `$desktop`, mais pas un `.ts` — la
 * transformation y cherche le `tsconfig.json` de la racine, qui étend un
 * fichier engendré par `svelte-kit sync` et donc absent d'un dépôt frais. Le
 * test aurait été vert ou rouge selon l'ordre des commandes, ce qui est pire
 * que pas de test.
 */

/** Le minimum qu'un profil doit porter pour être affiché ou caché. */
export interface DisplayableProfile {
  id: string;
  name: string;
  tcp_ports: number[];
  udp_ports: number[];
  builtin?: boolean;
  from_hub?: boolean;
}

/**
 * Un profil livré avec l'application ?
 *
 * ⚠️ Les deux critères, pas un seul : l'interface pose `builtin: true`, mais
 * une liste écrite par une version antérieure peut ne porter que le préfixe
 * d'identifiant. Se fier à un seul en laisserait passer la moitié.
 */
export function isBuiltinProfile(profile: DisplayableProfile): boolean {
  return profile.builtin === true || profile.id.startsWith('builtin:');
}

/**
 * Les profils à montrer, selon que la sonde est rattachée à un hub ou non.
 *
 * Rattachée : ce qu'elle a créé, et ce que le hub lui a livré. Seule : tout,
 * profils de base compris.
 *
 * ⚠️ Un profil créé localement puis monté au hub revient marqué `from_hub` sur
 * le MÊME identifiant — le hub réutilise celui que la sonde a donné. Il n'y a
 * donc rien à dédoublonner ici, et surtout pas par le nom : deux profils
 * différents peuvent légitimement en partager un.
 */
export function visibleProfiles<T extends DisplayableProfile>(all: T[], enrolled: boolean): T[] {
  if (!enrolled) return all;
  return all.filter(profile => !isBuiltinProfile(profile));
}

/**
 * Le profil actif parmi ceux qui sont visibles.
 *
 * 🔴 Le repli sur le premier visible n'est pas cosmétique : une sonde qu'on
 * vient d'enrôler garde `builtin:common` comme profil actif, devenu invisible.
 * Sans repli, le sélecteur n'afficherait aucune sélection et le scan partirait
 * avec une liste de ports que l'écran ne montre plus.
 *
 * ⚠️ Rend `undefined` quand il n'y a plus aucun profil, et c'est à l'appelant
 * de le traduire en « la sonde garde sa propre liste ». Fabriquer ici un profil
 * à zéro port ferait scanner TOUS les ports.
 */
export function activeProfile<T extends DisplayableProfile>(
  visible: T[],
  activeId: string | null | undefined,
): T | undefined {
  return visible.find(profile => profile.id === activeId) ?? visible[0];
}
