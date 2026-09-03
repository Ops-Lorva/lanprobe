import { describe, expect, it } from 'vitest';
import fr from './fr.json';
import en from './en.json';
import es from './es.json';

/**
 * Les trois traductions doivent porter EXACTEMENT les mêmes clés.
 *
 * ⚠️ Une clé absente ne casse rien : `svelte-i18n` affiche la clé brute. On
 * découvre donc « report.ipSheet » comme nom d'onglet Excel devant le client,
 * jamais en développement — l'espagnol avait déjà pris neuf clés de retard sur
 * le français sans qu'aucun test ne le dise.
 */
function flatten(node: unknown, prefix = ''): string[] {
  if (typeof node !== 'object' || node === null) return [prefix];
  return Object.entries(node as Record<string, unknown>).flatMap(([k, v]) =>
    flatten(v, prefix ? `${prefix}.${k}` : k),
  );
}

const CATALOGS: Record<string, unknown> = { fr, en, es };

describe('catalogues de traduction', () => {
  const reference = flatten(fr).sort();

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les mêmes clés que le français`, () => {
      expect(flatten(catalog).sort()).toEqual(reference);
    });
  }

  // Les clés que cette fonctionnalité introduit. Les nommer une à une plutôt
  // que se fier à la seule parité : une clé oubliée dans les TROIS fichiers
  // passerait la parité sans que rien ne s'affiche.
  const REQUISES = [
    'probe.ipHistoryEmpty',
    'probe.ipTable.label',
    'probe.ipTable.address',
    'probe.ipTable.gateway',
    'probe.ipTable.period',
    'probe.ipTable.duration',
    'probe.ipTable.uptime',
    'probe.ipTable.undetermined',
    'probe.ipTable.save',
    'probe.ipTable.error',
    'settings.proxies_title',
    'settings.proxies_label',
    'settings.proxies_hint',
  ];

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les clés du SLA par IP publique`, () => {
      const keys = new Set(flatten(catalog));
      expect(REQUISES.filter((k) => !keys.has(k))).toEqual([]);
    });
  }

  // Mode temps réel : le bouton, le repère du parc et le réglage de durée.
  // ⚠️ `probe.realtime_hint` et `probe.realtime_active` ne sont pas décoratifs :
  // ce sont eux qui annoncent la minute d'attente avant que la sonde change de
  // cadence. Sans eux, on croit à une panne et on reclique.
  const REQUISES_TEMPS_REEL = [
    'probe.realtime',
    'probe.realtime_stop',
    'probe.realtime_hint',
    'probe.realtime_active',
    'fleet.realtime_flag',
    'fleet.realtime_help',
    'settings.realtime_label',
    'settings.realtime_unit',
    'settings.realtime_hint',
    'settings.invalid_realtime',
    // Onglet « Temps réel » : les trois réglages se règlent ensemble, la durée
    // n'est plus seule sur « Général ».
    'settings.tab_realtime',
    'settings.realtime_title',
    'settings.realtime_lead',
    'settings.realtime_window_label',
    'settings.realtime_window_hint',
    'settings.realtime_heartbeat_label',
    'settings.realtime_heartbeat_hint',
    'settings.realtime_delay_note',
    'settings.invalid_realtime_window',
    // ⚠️ Le plancher de 5 s doit se DIRE, pas seulement se refuser : un champ
    // qui rejette sans expliquer se relit trois fois avant qu'on comprenne.
    'settings.invalid_realtime_heartbeat',
  ];

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les clés du mode temps réel`, () => {
      const keys = new Set(flatten(catalog));
      expect(REQUISES_TEMPS_REEL.filter((k) => !keys.has(k))).toEqual([]);
    });
  }

  // Un graphe par cible sur les onglets dédiés : chacun porte le nom de sa
  // cible et rappelle qu'il a sa propre échelle.
  const REQUISES_COURBES_PAR_CIBLE = ['charts.latency_of', 'charts.own_scale'];

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les clés des courbes par cible`, () => {
      const keys = new Set(flatten(catalog));
      expect(REQUISES_COURBES_PAR_CIBLE.filter((k) => !keys.has(k))).toEqual([]);
    });
  }

  // Les cartes en grille remplacent les tableaux de tête des onglets
  // Surveillance et Débit. Ce sont elles qui portent désormais les deux états
  // que le tableau ne disait pas : une cible ajoutée qui attend son premier
  // relevé, et une cible mesurée que la sonde ne surveille plus.
  const REQUISES_CARTES = [
    'probe.monitoring_awaiting',
    'probe.monitoring_dropped',
    'probe.monitoring_dropped_hint',
    'probe.monitor_stats_hint',
    'probe.speedtest_last',
    'probe.speedtest_engine_none',
    'charts.speed_shared_scale',
  ];

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les clés des cartes par cible et par moteur`, () => {
      const keys = new Set(flatten(catalog));
      expect(REQUISES_CARTES.filter((k) => !keys.has(k))).toEqual([]);
    });
  }

  // État « silencieuse » : la sonde n'a plus battu depuis plus d'une cadence,
  // mais pas encore assez pour être déclarée hors ligne. Sans ces clés, l'écran
  // retomberait sur le vert « en ligne », qui est exactement l'affirmation
  // fausse que cet état existe pour supprimer.
  // ⚠️ `fleet.rail_silent` fait partie du lot : le bandeau du parc comptait
  // encore sur `p.status` sous l'intitulé « sans nouvelles ». Deux mots pour le
  // même ensemble de sondes, et une sonde affichée « Silencieuse » sur sa ligne
  // était comptée « En ligne » dans le bandeau.
  const REQUISES_SILENCE = [
    'status.silent',
    'status.silent_for',
    'status.silent_help',
    'fleet.rail_silent',
  ];

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les clés de l’état silencieux`, () => {
      const keys = new Set(flatten(catalog));
      expect(REQUISES_SILENCE.filter((k) => !keys.has(k))).toEqual([]);
    });
  }

  // InfluxDB et Sauvegardes ont divorcé : la base de mesures et la politique
  // d'archivage répondent à deux questions différentes — « mes mesures
  // arrivent-elles ? » et « que me reste-t-il si cette machine disparaît ? ».
  // Mêlées, on cherchait la seconde en scrollant la première.
  const REQUISES_STOCKAGE = ['settings.tab_backups', 'settings.tab_measures'];

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les clés des onglets Mesures et Sauvegardes`, () => {
      const keys = new Set(flatten(catalog));
      expect(REQUISES_STOCKAGE.filter((k) => !keys.has(k))).toEqual([]);
    });
  }

  // Sélecteur de fenêtre sur TOUS les onglets, plage de dates comprise.
  // ⚠️ Les quatre messages de refus sont nommés un à un : un plafond qui
  // refuse sans dire pourquoi se lit comme une panne de l'interface, ce qui
  // est exactement ce que le plafond sert à éviter.
  const REQUISES_FENETRE = [
    'probe.range_minutes',
    'probe.window_apply',
    'probe.window_max_hint',
    'probe.window_incomplete',
    'probe.window_reversed',
    'probe.window_future',
    'probe.window_too_wide',
  ];

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les clés du sélecteur de fenêtre`, () => {
      const keys = new Set(flatten(catalog));
      expect(REQUISES_FENETRE.filter((k) => !keys.has(k))).toEqual([]);
    });
  }

  // Fenêtre de 10 min : en mode temps réel, la plus courte des fenêtres
  // existantes (1 h) écrase ce qui vient de se passer.
  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte le libellé de la fenêtre de 10 min`, () => {
      expect(new Set(flatten(catalog)).has('probe.range_10m')).toBe(true);
    });
  }

  // Retrait d'une surveillance DEPUIS LE HUB.
  // ⚠️ Le retrait arrête la surveillance d'une machine : la confirmation doit
  // nommer la cible et dire qu'elle est réversible. Une confirmation muette
  // sur la réversibilité fait renoncer à un geste sans danger.
  const REQUISES_RETRAIT = [
    'probe.monitoring_remove_title',
    'probe.monitoring_remove_body',
    'probe.monitoring_remove_reversible',
    'probe.monitoring_remove_confirm',
    'probe.monitoring_remove_working',
    'probe.monitoring_remove_done',
    'probe.monitoring_remove_moot',
    // 🔴 Une cible retirée ne figure plus sous « Actives » : sa carte et sa
    // courbe sont rangées sous « Retirées ». Sans ces deux libellés, cet
    // historique déplacé arriverait sans rien qui dise ce qu'il est.
    'probe.monitoring_removed_flag',
    'probe.monitoring_removed_history',
  ];

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les clés du retrait de surveillance`, () => {
      const keys = new Set(flatten(catalog));
      expect(REQUISES_RETRAIT.filter((k) => !keys.has(k))).toEqual([]);
    });
  }

  // Taux de disponibilité par cible, affiché sur la fiche.
  // ⚠️ `uptime_gap` dit quelle part de la fenêtre n'a AUCUN relevé. Sans lui,
  // le taux se lit « tout va bien depuis six heures » quand il dit « tout
  // allait bien pendant les vingt-huit minutes mesurées ».
  // ⚠️ `col_uptime` et `monitor_samples` nomment les valeurs de la SONDE, à
  // côté de celles de la période : deux nombres proches et différents sans
  // libellé qui les sépare se lisent comme une erreur.
  // 🔴 `monitor_stats_loading` distingue « je charge » de « il n'y a rien ».
  // Le bloc restait VIDE le temps que le parc réponde, et un vide se lit comme
  // une absence : on a cru la fonctionnalité supprimée.
  const REQUISES_UPTIME = [
    'probe.monitor_stats_loading',
    'probe.uptime_period',
    'probe.uptime_samples',
    'probe.uptime_gap',
    'probe.col_uptime',
    'probe.monitor_samples',
    'sla.undetermined',
  ];

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les clés du taux par cible`, () => {
      const keys = new Set(flatten(catalog));
      expect(REQUISES_UPTIME.filter((k) => !keys.has(k))).toEqual([]);
    });
  }

  // Agrégation : le pas et le maximum de l'intervalle.
  // ⚠️ Sans ces deux libellés, l'écran trace une moyenne de trois heures sans
  // le dire — une courbe crédible dont personne ne sait qu'elle est moyennée.
  const REQUISES_AGREGATION = ['charts.aggregated', 'charts.peak_label'];

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les clés de l'agrégation`, () => {
      const keys = new Set(flatten(catalog));
      expect(REQUISES_AGREGATION.filter((k) => !keys.has(k))).toEqual([]);
    });
  }

  // Appairage d'un téléphone et écran « Appareils » (contrat § 22).
  //
  // 🔴 `account.password_no_logout` n'est pas décorative : sur le web, changer
  // son mot de passe est le geste réflexe de « j'ai perdu mon téléphone ». Ici
  // il ne déconnecte AUCUN appareil appairé. Sans cette phrase, on fabrique
  // quelqu'un qui se croit protégé et ne l'est pas.
  //
  // ⚠️ `account.devices_last_seen_raw` dit « vu il y a 41 j » et rien d'autre :
  // il n'y a aucune révocation par inactivité, et un libellé qui laisserait
  // croire à une expiration automatique promettrait un filet qui n'existe pas.
  const REQUISES_APPAREILS = [
    'account.devices_title',
    'account.devices_lead',
    'account.devices_empty',
    'account.devices_pair_cta',
    'account.devices_code_title',
    'account.devices_code_hint',
    // 🔴 Le QR porte un secret qui TRANSITE : qui le photographie par-dessus
    // l'épaule appaire son propre téléphone, et cet appareil-là ne s'en ira
    // qu'à une révocation explicite. L'écran doit le dire ; le passer sous
    // silence laisse afficher un code d'accès permanent en réunion.
    'account.devices_code_shoulder',
    'account.devices_code_expires',
    'account.devices_code_expired',
    'account.devices_code_regen',
    'account.devices_manual_hint',
    'account.devices_fingerprint',
    'account.devices_fingerprint_none',
    'account.devices_col_name',
    'account.devices_col_platform',
    // La date d'appairage : c'est elle qui rattache une ligne à un geste dont
    // on se souvient — « le téléphone que j'ai appairé lundi ». Sans elle, deux
    // appareils du même modèle ne se distinguent que par un nom saisi à la main.
    'account.devices_col_paired',
    'account.devices_col_last_seen',
    'account.devices_col_last_ip',
    'account.devices_last_seen_raw',
    'account.devices_never_seen',
    'account.devices_rename',
    'account.devices_revoke',
    'account.devices_revoke_confirm',
    'account.devices_revoked',
    'account.devices_rotate',
    'account.devices_rotate_hint',
    'account.devices_all_title',
    'account.devices_owner',
    'account.password_no_logout',
  ];

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les clés de l'appairage`, () => {
      const keys = new Set(flatten(catalog));
      expect(REQUISES_APPAREILS.filter((k) => !keys.has(k))).toEqual([]);
    });
  }

  // Rapports produits par le hub (contrat § 23).
  //
  // 🔴 `report.hub_purged` et `report.hub_purged_hint` portent la distinction
  // qui fait tout : le fichier s'en va, l'entrée reste, avec son demandeur.
  // Sans elles, une ligne sans bouton de téléchargement se lit comme une panne.
  //
  // ⚠️ `report.hub_truncated` dit LES DEUX NOMBRES — « attendu 248 013 o · reçu
  // 91 220 o ». Un « erreur réseau » générique ne permet pas de décider s'il
  // faut réessayer ou appeler.
  //
  // ⚠️ `report.hub_served` et non « téléchargé » : le hub sait qu'il a servi le
  // fichier, il ne sait pas que le téléphone l'a reçu.
  const REQUISES_RAPPORTS_HUB = [
    'report.hub_tab',
    'report.hub_lead',
    'report.hub_generate',
    'report.hub_generate_hint',
    'report.hub_window',
    'report.hub_probes',
    'report.hub_probes_all',
    'report.hub_locale_hint',
    'report.hub_preparing',
    'report.hub_step',
    'report.hub_ready',
    'report.hub_failed',
    'report.hub_download',
    'report.hub_served',
    'report.hub_purged',
    'report.hub_purged_hint',
    'report.hub_request_again',
    'report.hub_expires_at',
    'report.hub_requested_by',
    'report.hub_requested_from_web',
    'report.hub_size',
    'report.hub_empty',
    'report.hub_truncated',
    // ⚠️ Un corps de la bonne LONGUEUR dont l'empreinte diffère n'est pas un
    // téléchargement tronqué : c'est le volume du hub qui a bougé sous le
    // fichier. Deux causes, deux gestes — réessayer ne sert à rien sur la
    // seconde.
    'report.hub_corrupted',
    // Ni longueur ni empreinte annoncées : il n'y a rien à comparer, et un
    // classeur dont on ne peut rien affirmer ne s'enregistre pas.
    'report.hub_unverifiable',
    // Le hub est plus ancien que la route : le classeur a été produit par le
    // navigateur. Le dire, sinon deux documents identiques à l'œil sortent de
    // deux générateurs sans que personne sache lequel.
    'report.hub_fallback',
    'report.hub_history',
    'settings.report_days_title',
    'settings.report_days_lead',
    'settings.report_days_label',
    'settings.report_days_hint',
    'settings.report_days_purge_count',
    'settings.invalid_report_days',
  ];

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les clés des rapports du hub`, () => {
      const keys = new Set(flatten(catalog));
      expect(REQUISES_RAPPORTS_HUB.filter((k) => !keys.has(k))).toEqual([]);
    });
  }
});
