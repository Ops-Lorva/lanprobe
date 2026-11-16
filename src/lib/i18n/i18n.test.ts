import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath, URL } from 'node:url';
import { describe, expect, it } from 'vitest';

/**
 * Les textes de l'app desktop.
 *
 * Trois langues, une seule liste de clés : une clé absente d'une langue
 * s'affiche en anglais (repli de svelte-i18n) au milieu d'un écran français,
 * et une clé absente partout s'affiche telle quelle (« monitoring.title »).
 */
const ici = (rel: string) => fileURLToPath(new URL(rel, import.meta.url));
const lire = (rel: string) => readFileSync(ici(rel), 'utf8');

type Arbre = { [k: string]: string | Arbre };
function aplatir(o: Arbre, p = ''): Map<string, string> {
  const out = new Map<string, string>();
  for (const [k, v] of Object.entries(o)) {
    if (typeof v === 'string') out.set(p + k, v);
    else for (const [kk, vv] of aplatir(v, `${p}${k}.`)) out.set(kk, vv);
  }
  return out;
}
const LANGS = {
  en: aplatir(JSON.parse(lire('./en.json'))),
  fr: aplatir(JSON.parse(lire('./fr.json'))),
  es: aplatir(JSON.parse(lire('./es.json'))),
};

const COMPOSANTS = readdirSync(ici('../components'))
  .filter((f) => f.endsWith('.svelte'))
  .map((f) => ({ f, src: lire(`../components/${f}`) }))
  .concat([{ f: '+page.svelte', src: lire('../../routes/+page.svelte') }]);

describe('les clés', () => {
  it('sont les mêmes dans les trois langues', () => {
    const en = [...LANGS.en.keys()].sort();
    expect([...LANGS.fr.keys()].sort()).toEqual(en);
    expect([...LANGS.es.keys()].sort()).toEqual(en);
  });

  // Les clés écrites en dur dans les composants : `$_('a.b')`.
  it('appelées par les écrans existent toutes', () => {
    const manquantes: string[] = [];
    for (const { f, src } of COMPOSANTS) {
      for (const m of src.matchAll(/\$_\(\s*'([a-z0-9_.]+)'/g)) {
        if (!LANGS.en.has(m[1])) manquantes.push(`${f}: ${m[1]}`);
      }
    }
    expect(manquantes).toEqual([]);
  });
});

describe('le français', () => {
  // L'anglais resté en dur dans les valeurs françaises : relevé à l'audit.
  const ANGLAIS = [
    'Monitoring', 'Discovery', 'Port Scan', 'Speed Test', 'DOWN', 'OPEN', 'Hostname',
    'Sidebar', 'download', 'upload', 'jitter', 'uptime', 'Custom', 'Ping Monitor', 'closed',
    'probes', 'avg', 'p95', 'Dashboard',
  ];
  it('ne garde aucun mot anglais', () => {
    const fautes: string[] = [];
    for (const [k, v] of LANGS.fr) {
      for (const mot of ANGLAIS) {
        if (new RegExp(`\\b${mot}\\b`).test(v)) fautes.push(`${k} = ${v}`);
      }
    }
    expect(fautes).toEqual([]);
  });
});

describe('les écrans', () => {
  // Des libellés visibles écrits en dur : ils restaient anglais en français.
  const EN_DUR = ['>Public IP<', "'Public IP'", '>Headless<', "'Coupure'", "'Aucune donnée'", '🌐', '🖥', '📡', '🔍'];
  it('ne contiennent pas de libellé en dur', () => {
    const fautes: string[] = [];
    for (const { f, src } of COMPOSANTS) {
      for (const t of EN_DUR) if (src.includes(t)) fautes.push(`${f}: ${t}`);
    }
    expect(fautes).toEqual([]);
  });
});
