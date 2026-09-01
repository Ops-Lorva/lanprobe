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
});
