/**
 * La date de scan **par machine** (contrat § 12).
 *
 * 🔴 Depuis que la sonde publie toutes les machines qu'elle connaît à chaque
 * scan de ports — sans quoi le hub, qui n'affiche que le dernier scan, perdait
 * les précédentes —, `started_at` date le **LOT** publié. L'afficher sur une
 * ligne de machine fait passer une machine scannée il y a une heure pour
 * scannée à l'instant : une valeur plausible et fausse.
 *
 * ⚠️ Une machine **sans** date reste sans date. Les lignes d'avant la v28 du
 * schéma n'en ont pas, et une sonde antérieure n'en envoie pas : l'écran écrit
 * « date inconnue ». Inventer « maintenant » serait le mensonge qu'on corrige.
 */

import type { ScanHost } from './api';

/**
 * Les dates de scan par adresse, en **une passe**.
 *
 * ⚠️ Un scan d'un /24 donne des centaines de machines. Chercher la date dans le
 * tableau `hosts` à chaque ligne affichée ferait un parcours par ligne ; cette
 * table se construit une fois pour la page.
 *
 * Une entrée absente de la table veut dire « date inconnue » — y compris pour
 * un `0`, qui daterait de 1970 et n'est qu'une absence déguisée.
 */
export function hostScanDates(hosts: ScanHost[] | null | undefined): Map<string, number> {
  const out = new Map<string, number>();
  for (const host of hosts ?? []) {
    if (typeof host.scanned_at === 'number' && host.scanned_at > 0) {
      out.set(host.ip, host.scanned_at);
    }
  }
  return out;
}
