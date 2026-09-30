/**
 * Chercher une machine dans une découverte.
 *
 * 🔴 Sur un /24 bien rempli, la liste dépasse la centaine. On y cherche une
 * adresse ou une MAC relevée sur une étiquette de baie — et on la tape comme
 * on l'a lue.
 *
 * ⚠️ La MAC se compare SANS ponctuation : « 94:2a:6f » et « 942a6f » désignent
 * la même machine, et exiger la bonne ponctuation ferait conclure « pas
 * trouvée » à tort.
 *
 * ⚠️ Filtre ce qui a DÉJÀ été vu : aucun paquet ne part pendant qu'on tape.
 *
 * Même règle que l'app iOS (`Sources/LanProbeCore/DiscoverySearch.swift`) :
 * deux surfaces du même produit ne cherchent pas de deux façons.
 */
export interface SearchableHost {
  ip: string;
  hostname?: string | null;
  mac?: string | null;
  vendor?: string | null;
}

const hexOnly = (value: string) => value.toLowerCase().replace(/[^0-9a-f]/g, '');

export function matchingHosts<T extends SearchableHost>(hosts: T[], query: string): T[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return hosts;
  const loose = hexOnly(needle);
  return hosts.filter((h) => {
    if (h.ip.toLowerCase().includes(needle)) return true;
    if (h.hostname?.toLowerCase().includes(needle)) return true;
    if (h.vendor?.toLowerCase().includes(needle)) return true;
    if (h.mac && loose && hexOnly(h.mac).includes(loose)) return true;
    return false;
  });
}
