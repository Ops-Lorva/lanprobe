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
  ];

  for (const [lang, catalog] of Object.entries(CATALOGS)) {
    it(`${lang} porte les clés du SLA par IP publique`, () => {
      const keys = new Set(flatten(catalog));
      expect(REQUISES.filter((k) => !keys.has(k))).toEqual([]);
    });
  }
});
