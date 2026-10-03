/**
 * Les machines d'un scan de ports, **chacune avec ses ports ouverts**.
 *
 * 🔴 **On part des MACHINES, et les ports s'y rattachent.** L'écran et les deux
 * générateurs de classeur groupaient par `ports`, l'inventaire à plat : une
 * machine scannée dont aucun port n'est ouvert n'apparaissait alors nulle part,
 * alors que la sonde l'avait bel et bien publiée dans `hosts`. Elle était donc
 * indiscernable d'une machine jamais scannée — et c'est pourtant le résultat le
 * plus rassurant qu'on puisse remettre à un client : « scannée, rien d'ouvert ».
 *
 * ⚠️ **Sans jamais inventer de machine.** Une adresse absente de `hosts` ET
 * sans port ouvert n'a pas été scannée : elle reste absente. Dire « aucun port
 * ouvert » d'une machine que personne n'a interrogée serait pire que le silence
 * qu'on corrige.
 *
 * ⚠️ **Une machine dont on ne connaît que les ports compte quand même.** Une
 * sonde d'avant la v28 du schéma publie ses ports sans publier ses machines
 * (`hosts` vide, `ports` plein). Partir strictement de `hosts` ferait
 * disparaître de l'écran et du classeur des ports ouverts bel et bien relevés.
 *
 * ⚠️ Ces règles vivent ici et pas dans le `.svelte` ni dans le générateur : le
 * projet ne monte pas de composants, et une règle écrite trois fois diverge dès
 * la première retouche. C'est le même partage que `hostScanDates` et
 * `scanProfileLabel`, et la jumelle côté hub est `machines_du_scan`
 * (`crates/lanprobe-web/src/report_xlsx.rs`).
 */

import type { ScanHost, ScanPort } from './api';

/** Une machine scannée, et les ports qu'on lui a trouvés ouverts. */
export interface PortScanHost {
  ip: string;
  /** Vide = scannée, aucun port ouvert. Jamais « on ne sait pas ». */
  ports: ScanPort[];
}

/**
 * Les machines du scan, dans **l'ordre de la sonde** : les siennes d'abord,
 * puis celles que seuls les ports révèlent.
 *
 * ⚠️ Volontairement NON trié. Le classeur veut l'ordre de la sonde — trier ici
 * changerait l'ordre des lignes de documents déjà remis à des clients — et
 * l'écran applique le sien par-dessus.
 *
 * ⚠️ En une passe : un scan d'un /24 donne des centaines de machines, et
 * filtrer la liste des ports machine par machine ferait un parcours complet par
 * ligne affichée.
 */
export function portScanHosts(
  scan: { hosts?: ScanHost[] | null; ports?: ScanPort[] | null } | null | undefined,
): PortScanHost[] {
  const parIp = new Map<string, PortScanHost>();
  for (const h of scan?.hosts ?? []) {
    if (!parIp.has(h.ip)) parIp.set(h.ip, { ip: h.ip, ports: [] });
  }
  for (const p of scan?.ports ?? []) {
    let ligne = parIp.get(p.ip);
    if (!ligne) {
      ligne = { ip: p.ip, ports: [] };
      parIp.set(p.ip, ligne);
    }
    ligne.ports.push(p);
  }
  return [...parIp.values()];
}
