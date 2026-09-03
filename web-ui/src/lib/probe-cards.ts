/**
 * Cartes des onglets Surveillance et Débit de la fiche d'une sonde.
 *
 * ⚠️ Ce découpage vit ici, et pas dans le `.svelte`, parce que le projet ne
 * monte pas de composants : une règle écrite dans le gabarit ne serait
 * couverte par rien. Or les deux règles ci-dessous sont exactement celles
 * qu'on a déjà cassées à l'écran — une cible qu'on vient d'ajouter et qui
 * n'apparaît nulle part, une cible retirée montrée comme si elle était encore
 * surveillée.
 */

import type { MonitorState, SpeedtestRow } from './api';
import { SERIES, engineLabel, type Serie } from './charts';

/** Courbe de latence telle que la fiche la prépare (`pingCurves`). */
export interface LatencyCurve extends Serie {
  /** Adresse pinguée, `''` si la courbe n'en nomme aucune. */
  target: string;
}

export interface MonitorCard {
  /** Clé de rendu, distincte d'une carte à l'autre. */
  key: string;
  target: string;
  /**
   * Ce que la sonde annonce surveiller MAINTENANT. `null` quand elle ne
   * l'annonce plus — ou quand elle n'annonce rien du tout.
   */
  monitor: MonitorState | null;
  /** Ce qui a été mesuré pendant la fenêtre. `null` avant le premier relevé. */
  curve: LatencyCurve | null;
  /**
   * Mesurée sur la fenêtre, plus annoncée par la sonde, et le hub n'enregistre
   * AUCUN retrait pour elle : on constate qu'elle a cessé, on ne sait pas
   * pourquoi. Le drapeau dit ce qu'on observe, pas ce qu'on suppose.
   */
  stale: boolean;
  /**
   * Le hub porte un retrait daté pour cette cible.
   *
   * ⚠️ Exclusif de `stale` : quand le retrait est connu, il n'y a rien à
   * supposer. C'est ce drapeau qui range la carte sous « Retirées » — jamais
   * sous « Actives », quelle que soit la fenêtre affichée.
   */
  removed: boolean;
}

/**
 * Fusionne les surveillances annoncées et les courbes mesurées, sans les
 * confondre.
 *
 * ⚠️ Deux faits différents. La sonde annonce à chaque battement ce qu'elle
 * surveille MAINTENANT ; les mesures montrent ce qui a tourné pendant la
 * fenêtre affichée. Ne rendre que les courbes perdrait la cible qu'on vient
 * d'ajouter, qui n'a pas encore de point ; ne rendre que l'annonce ferait
 * disparaître une cible retirée dont les relevés sont encore à l'écran.
 *
 * `monitors === null` = sonde antérieure à cette annonce. Rien n'est alors
 * marqué comme retiré : conclure au retrait de toutes ses cibles serait faux.
 *
 * 🔴 `removedTargets` est la troisième source, et la seule qui TRANCHE. Les
 * cartes se construisaient depuis les mesures de la fenêtre : une cible
 * retirée y restait tant que ses points y étaient — sous « Actives », avec un
 * drapeau qui contredisait l'onglet. « Elle partira dans dix minutes » n'est
 * pas une réponse quand la fenêtre fait sept jours. Sa carte n'est pas perdue
 * pour autant : elle porte `removed`, et l'appelant la range sous « Retirées »
 * avec sa courbe.
 */
export function monitorCards(
  monitors: MonitorState[] | null,
  curves: LatencyCurve[],
  removedTargets: Iterable<string> = [],
): MonitorCard[] {
  const removed = new Set(removedTargets);
  const byTarget = new Map(curves.filter((c) => c.target).map((c) => [c.target, c]));
  const placed = new Set<string>();
  const cards: MonitorCard[] = [];

  for (const m of monitors ?? []) {
    const curve = byTarget.get(m.ip) ?? null;
    if (curve) placed.add(curve.key);
    cards.push({
      key: `t:${m.ip}`,
      target: m.ip,
      monitor: m,
      curve,
      stale: false,
      // ⚠️ Le retrait est un fait daté, l'annonce un état qui n'a pas encore
      // reçu l'ordre — le hub ne joint jamais la sonde. C'est donc le fait qui
      // gagne, ici comme dans `effectiveMonitors` : la règle ne doit pas
      // dépendre de l'appelant qui la précède.
      removed: removed.has(m.ip),
    });
  }

  for (const c of curves) {
    if (placed.has(c.key)) continue;
    const gone = c.target !== '' && removed.has(c.target);
    cards.push({
      key: c.target ? `t:${c.target}` : `c:${c.key}`,
      target: c.target,
      monitor: null,
      curve: c,
      // Une courbe anonyme ne se rapproche d'aucune surveillance annoncée :
      // on ne peut donc rien affirmer sur son retrait. Et quand le retrait est
      // connu, il n'y a plus rien à supposer : les deux drapeaux s'excluent.
      stale: monitors !== null && c.target !== '' && !gone,
      removed: gone,
    });
  }

  return cards;
}

/** Courbe de débit telle que la fiche la prépare (`speedCurves`). */
export interface SpeedCurve extends Serie {
  /** Étiquette Influx du moteur — `ookla`, `iperf3`, `''` si absente. */
  engine: string;
  /** Champ Influx : c'est lui qui dit le sens, donc la couleur. */
  field: string;
  /** Sens seul — « Descendant », « Montant » — sans le nom du moteur. */
  direction: string;
}

export interface SpeedCard {
  /** Moteur normalisé, clé de rendu. */
  key: string;
  /** Nom affichable — titre de la carte. `''` si le moteur n'est pas nommé. */
  label: string;
  /** Descendant et montant, sur la même échelle. */
  series: SpeedCurve[];
  /** Dernier test de CE moteur, `null` s'il ne figure pas dans l'inventaire. */
  last: SpeedtestRow | null;
}

/**
 * Une carte par moteur, descendant et montant réunis dedans.
 *
 * ⚠️ Descendant et montant ne sont PAS séparés ici, contrairement aux cibles
 * de latence : ce sont deux débits de même unité et de même ordre de grandeur,
 * une échelle commune ne les écrase pas — et c'est précisément l'un contre
 * l'autre qu'on les lit. L'argument de l'échelle propre vaut pour un hôte LAN
 * à 1 ms face à un hôte WAN à 200 ms, pas ici.
 *
 * ⚠️ La couleur est refixée par le sens. Celle de `speedCurves` est attribuée
 * au rang, pour un graphe unique où quatre courbes doivent se distinguer ;
 * éclatée en cartes, elle ferait changer « descendant » de teinte selon le
 * moteur — la seule chose que la couleur devait fixer.
 */
export function speedCards(curves: SpeedCurve[], history: SpeedtestRow[]): SpeedCard[] {
  const order: string[] = [];
  const seen = new Set<string>();
  const remember = (raw: string | null | undefined) => {
    const key = (raw ?? '').toLowerCase();
    if (seen.has(key)) return;
    seen.add(key);
    order.push(key);
  };
  for (const c of curves) remember(c.engine);
  // L'historique est lu ensuite : un moteur dont le dernier test est plus
  // ancien que la fenêtre n'a pas de courbe, mais il a bien tourné.
  for (const r of history) remember(r.engine);

  return order.map((key) => ({
    key,
    label: engineLabel(key),
    series: curves
      .filter((c) => (c.engine ?? '').toLowerCase() === key)
      .map((c) => ({
        ...c,
        // Le moteur titre déjà la carte : le répéter en légende écrirait deux
        // fois la même chose à trois centimètres d'écart.
        label: c.direction || c.label,
        color: c.field === 'download_mbps' ? SERIES.one : SERIES.two,
      })),
    // Le plus récent, pas le premier venu : l'ordre de l'inventaire n'est pas
    // un contrat, et une carte qui daterait le mauvais test serait indétectable.
    last: history
      .filter((r) => (r.engine ?? '').toLowerCase() === key)
      .reduce<SpeedtestRow | null>(
        (best, r) => (best === null || r.started_at > best.started_at ? r : best),
        null,
      ),
  }));
}
