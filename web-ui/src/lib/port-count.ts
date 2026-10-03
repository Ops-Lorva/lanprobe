/**
 * Le décompte des ports ouverts d'une machine, **avec leur famille**.
 *
 * 🔴 « 8 ports ouverts » ne dit pas ce qu'on regarde. Huit services TCP et une
 * machine qui ne répond qu'en UDP ne se vérifient pas de la même façon, et le
 * chiffre seul laisse le technicien deviner laquelle des deux il a sous les
 * yeux.
 *
 * 🔴 **La notation est celle de l'app iOS** (`PortScanWording.ports(total:tcp:udp:)`
 * dans `lanprobe-ios`), à la virgule près. Deux surfaces du même produit qui
 * comptent de deux façons font douter des deux — et c'est le genre d'écart qui
 * naît quand la règle est écrite à deux endroits.
 *
 * ⚠️ **Zéro port ouvert n'est plus un décompte mais un constat** depuis que
 * l'écran montre les machines scannées sans port ouvert : l'app iOS, qui
 * groupait elle aussi par `scan.ports`, a la même correction à faire.
 *
 * ⚠️ Ces règles vivent ici et pas dans le `.svelte` : le projet ne monte pas de
 * composants, une règle écrite dans un gabarit ne serait couverte par rien.
 */

type Translate = (key: string, values?: Record<string, string | number>) => string;

/** Combien de ports ouverts, et de quelle famille. */
export interface PortFamilies {
  /**
   * Tous les ports ouverts.
   *
   * ⚠️ Il peut **DÉPASSER** `tcp + udp` : un protocole qu'on ne sait pas nommer
   * compte quand même, le port est ouvert. C'est pour ça que le total est un
   * champ à part et non une somme calculée à l'affichage.
   */
  total: number;
  tcp: number;
  udp: number;
}

/**
 * Les familles d'une liste de ports, en **une passe**.
 *
 * ⚠️ Un scan d'un /24 donne des centaines de machines : trois filtrages par
 * ligne affichée feraient trois parcours par ligne.
 *
 * ⚠️ N'exige que le protocole, pas un `ScanPort` entier : l'écran groupe les
 * ports par machine et laisse tomber l'adresse au passage — elle est déjà sur
 * la ligne. Exiger le type complet aurait forcé à la recopier pour rien.
 */
export function countPortFamilies(ports: readonly { proto: string }[]): PortFamilies {
  const families = { total: ports.length, tcp: 0, udp: 0 };
  for (const p of ports) {
    // ⚠️ La sonde écrit « tcp » en minuscules, mais le protocole arrive de la
    // base : une casse différente ne doit pas faire disparaître un port d'une
    // famille pour le laisser dans le total.
    const proto = p.proto.toLowerCase();
    if (proto === 'tcp') families.tcp += 1;
    else if (proto === 'udp') families.udp += 1;
  }
  return families;
}

/**
 * Ce que la ligne d'une machine dit **sans qu'on l'ouvre**.
 *
 * ⚠️ Une seule famille ne se répète pas : « 2 ports · 2 TCP » dit deux fois la
 * même chose, on écrit « 2 ports TCP ouverts ». D'où les clés dédiées, qui
 * laissent chaque langue accorder la phrase entière.
 *
 * ⚠️ « TCP » et « UDP » ne se traduisent jamais : ce sont les noms des
 * protocoles. Seul le décompte s'accorde, et il faut le faire même si « port »
 * s'écrit pareil en français et en anglais.
 */
export function portCountWording(families: PortFamilies, t: Translate): string {
  const { total, tcp, udp } = families;
  // 🔴 Une machine scannée sans port ouvert dit ce qu'elle EST, pas un
  // décompte. « 0 port ouvert » se lit comme une case qu'on n'a pas su
  // remplir ; c'est au contraire un constat, et le plus rassurant qu'un client
  // puisse recevoir. Le classeur emploie la MÊME clé : deux formulations pour
  // le même cas laisseraient croire à deux cas.
  //
  // ⚠️ Cette garde passe aussi avant les familles : sans elle, `tcp === total`
  // serait vrai pour deux zéros et l'écran attribuerait au TCP une absence de
  // port ouvert.
  if (total === 0) return t('probe.ports_none_open');
  if (tcp === total) return t('probe.ports_count_tcp', { n: total });
  if (udp === total) return t('probe.ports_count_udp', { n: total });

  const parts = [t('probe.ports_count', { n: total })];
  // Une famille absente ne s'écrit pas : « · 0 UDP » ferait chercher ce qui
  // n'existe pas.
  if (tcp > 0) parts.push(`${tcp} TCP`);
  if (udp > 0) parts.push(`${udp} UDP`);
  return parts.join(' · ');
}
