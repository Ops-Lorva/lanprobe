/**
 * Parité des deux générateurs de classeur SLA — côté navigateur.
 *
 * 🔴 **Ce fichier et `crates/lanprobe-web/tests/parite_classeur.rs` forment une
 * seule épreuve.** Une charge utile figée (`tests/parite-classeur/*.json`) est
 * jouée par les deux générateurs ; chacun écrit son relevé canonique du
 * classeur produit, et les deux relevés doivent coïncider cellule par cellule.
 *
 * Pourquoi : le navigateur alimente les clients aujourd'hui, le hub prendra la
 * suite. Si les deux divergent, **personne ne le verra** — les deux rendent un
 * document crédible, et un client recevrait un chiffre différent du même mois
 * selon l'outil qui a produit le fichier.
 *
 * ⚠️ **Ce fichier est la RÉFÉRENCE.** Il produit le relevé
 * `releve-navigateur.json`, que le test Rust relit. Le régénérer se fait avec
 * `PARITE_CLASSEUR=maj npm run test:web` — et la sortie doit être relue, pas
 * subie : c'est le document remis au client qui change.
 *
 * ⚠️ **Les écarts connus sont énumérés** dans `ecarts-attendus.json`, un par
 * cellule. Le test Rust échoue aussi bien sur un écart NOUVEAU que sur un écart
 * DISPARU : dans les deux cas la bascule mérite une décision, pas un silence.
 *
 * ⚠️ **Aucune image ici.** `chartPng` rend `null` hors navigateur (`vitest`
 * tourne en `environment: 'node'`, sans `<canvas>`) : le classeur du navigateur
 * n'a PAS de graphique dans ce test alors qu'il en a en production. La parité
 * des images n'est donc **pas** établie par cette épreuve.
 */

// ⚠️ Avant tout usage d'`Intl` : le générateur du navigateur date dans le
// fuseau du POSTE. Sans fuseau figé, ce test rendrait un relevé différent selon
// la machine — et masquerait l'écart qu'il doit justement mesurer.
process.env.TZ = 'UTC';

import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it, beforeAll } from 'vitest';
import { addMessages, init, locale as i18nLocale, _ } from 'svelte-i18n';
import { get } from 'svelte/store';
import fr from './i18n/fr.json';
import en from './i18n/en.json';
import es from './i18n/es.json';
import { buildSlaWorkbook, type SlaPayload } from './sla-report';

const RACINE = fileURLToPath(new URL('../../../tests/parite-classeur/', import.meta.url));

/** Ce que la charge utile figée porte : de quoi produire le classeur des deux côtés. */
interface Charge {
  locale: string;
  site_name: string;
  range_start: number;
  range_stop: number;
  payloads: SlaPayload[];
}

/** Une cellule, telle qu'un tableur la relira. `null` = cellule vide. */
type Cellule = { s: string } | { n: number } | null;

interface FeuilleRelevee {
  nom: string;
  /** Indexées par référence Excel (« A1 »). Une cellule vide est absente. */
  cellules: Record<string, Cellule>;
  /** Largeur de chaque colonne, en caractères. */
  largeurs: number[];
  /** Nombre d'images posées sur la feuille. */
  images: number;
}

interface Releve {
  scenarios: Record<string, FeuilleRelevee[]>;
}

const SCENARIOS = ['solo', 'multi', 'noms'] as const;

function charge(nom: string): Charge {
  return JSON.parse(readFileSync(`${RACINE}charge-${nom}.json`, 'utf8')) as Charge;
}

/**
 * La traduction RÉELLE de l'interface, pas une imitation.
 *
 * ⚠️ Une fonction bouchon (`(k) => k`) prouverait la parité des clés et rien de
 * plus : c'est `svelte-i18n` qui rend « 7 derniers jours » à partir d'un
 * pluriel ICU, et c'est cette chaîne-là qui atterrit dans le classeur du
 * client. Le hub la reconstruit avec son propre interpolateur — comparer les
 * deux est précisément l'objet du test.
 */
function traducteur() {
  return (k: string, v?: Record<string, string | number>) => get(_)(k, { values: v }) as string;
}

/** Le relevé canonique d'un classeur exceljs, relu depuis les octets écrits. */
async function relever(payloads: SlaPayload[], lang: string): Promise<FeuilleRelevee[]> {
  const ExcelJS = (await import('exceljs')).default;
  const wb = await buildSlaWorkbook(payloads, traducteur(), lang);
  // ⚠️ On relit le FICHIER, pas les objets qui ont servi à l'écrire : un
  // générateur qui se vérifie sur son propre état interne ne prouve rien du
  // document remis au client.
  const octets = await wb.xlsx.writeBuffer();
  const relu = new ExcelJS.Workbook();
  await relu.xlsx.load(octets as ArrayBuffer);

  const out: FeuilleRelevee[] = [];
  relu.eachSheet((ws) => {
    const cellules: Record<string, Cellule> = {};
    ws.eachRow({ includeEmpty: false }, (row) => {
      row.eachCell({ includeEmpty: false }, (cell) => {
        const v = cell.value;
        // ⚠️ Une chaîne VIDE n'est pas relevée comme « rien » : `exceljs` écrit
        // bel et bien une cellule pour `''`, quand le hub n'en écrit aucune.
        // Un tableur les distingue (`ISBLANK`, `NB`), donc la comparaison doit
        // les distinguer aussi — les confondre masquerait un vrai écart.
        if (v === null || v === undefined) return;
        if (typeof v === 'number') cellules[cell.address] = { n: v };
        else if (typeof v === 'string') cellules[cell.address] = { s: v };
        else if (v instanceof Date) cellules[cell.address] = { s: v.toISOString() };
        else if (typeof v === 'object' && 'richText' in v)
          cellules[cell.address] = {
            s: (v as { richText: { text: string }[] }).richText.map((r) => r.text).join(''),
          };
        else cellules[cell.address] = { s: String(v) };
      });
    });
    const largeurs: number[] = [];
    ws.columns?.forEach((c) => largeurs.push(Math.round((c.width ?? 0) * 100) / 100));
    out.push({
      nom: ws.name,
      cellules,
      largeurs,
      images: (ws.getImages?.() ?? []).length,
    });
  });
  return out;
}

describe('parité du classeur SLA — relevé du navigateur', () => {
  let releve: Releve;

  beforeAll(async () => {
    addMessages('fr', fr);
    addMessages('en', en);
    addMessages('es', es);
    await init({ fallbackLocale: 'en', initialLocale: 'fr' });
    i18nLocale.set('fr');

    releve = { scenarios: {} };
    for (const nom of SCENARIOS) {
      const c = charge(nom);
      i18nLocale.set(c.locale);
      releve.scenarios[nom] = await relever(c.payloads, c.locale);
    }
  });

  it('écrit le relevé de référence que le hub devra reproduire', () => {
    const chemin = `${RACINE}releve-navigateur.json`;
    const rendu = JSON.stringify(releve, null, 2) + '\n';
    if (process.env.PARITE_CLASSEUR === 'maj') {
      writeFileSync(chemin, rendu, 'utf8');
    }
    // ⚠️ Sans mise à jour explicite, le relevé committé fait foi : c'est lui
    // que le test Rust compare. Un changement du générateur du navigateur doit
    // se voir ICI, dans une revue, avant d'atteindre le hub.
    expect(readFileSync(chemin, 'utf8')).toBe(rendu);
  });

  // ── Les invariants qui ne se négocient pas ────────────────────────────
  //
  // Ils sont vérifiés des DEUX côtés, avec les mêmes mots : une parité
  // parfaite sur deux classeurs également fautifs ne vaudrait rien.

  it('ne pose aucune ligne de total ni de moyenne', () => {
    const interdits = /^(total|totaux|moyenne|somme|cumul|average|sum)\b/i;
    for (const feuilles of Object.values(releve.scenarios)) {
      for (const f of feuilles) {
        for (const [ref, c] of Object.entries(f.cellules)) {
          if (c && 's' in c) {
            expect(
              interdits.test(c.s.trim()),
              `${f.nom}!${ref} = ${JSON.stringify(c.s)}`,
            ).toBe(false);
          }
        }
      }
    }
  });

  it('écrit l’indéterminé en CHAÎNE, jamais en nombre', () => {
    // Un tableur agrège ce qui est numérique : « indéterminé » rendu en nombre
    // ferait renaître, dans une somme de colonne, le chiffre que le générateur
    // a refusé de calculer.
    const synthese = releve.scenarios.solo[0];
    // Ligne 10, colonne C = la disponibilité de la cible entièrement sans
    // verdict (4e ligne de données : internet, 192.168.1.1, 8.8.8.8, muette).
    expect(synthese.cellules['C10']).toEqual({ s: 'indéterminé' });
    // Colonne J = Couverture : une chaîne quand elle est dite, RIEN sinon —
    // jamais un nombre qu'une moyenne de colonne pourrait aspirer.
    expect(synthese.cellules['J8']).toEqual({ s: '92.6 % de la période mesurée' });
    // Période entièrement mesurée : rien à dire, et surtout pas un zéro.
    // ⚠️ Le navigateur pose ici une cellule à chaîne VIDE — pas une cellule
    // absente. Le hub n'écrit rien du tout : c'est un écart réel, relevé dans
    // `ecarts-attendus.json`, invisible à l'œil mais pas à `NB()`.
    expect(synthese.cellules['J9']).toEqual({ s: '' });
    expect(synthese.cellules['J10']).toEqual({ s: 'période indéterminée' });
    // Et sur la feuille par adresse publique, la tranche non imputée.
    const adresses = releve.scenarios.solo.at(-1)!;
    expect(adresses.nom).toBe('SLA par IP publique');
    expect(adresses.cellules['C4']).toEqual({ s: 'Indéterminé' });
    // Un pourcentage en CHAÎNE ici aussi : la colonne ne doit pas se totaliser.
    expect(adresses.cellules['H4']).toEqual({ s: '100.00 %' });
  });

  it('n’embarque aucun graphique hors navigateur — la parité des images n’est pas prouvée ici', () => {
    // ⚠️ En production le classeur du navigateur PORTE des images. Ce test ne
    // peut pas les comparer : il n'y a pas de `<canvas>` sous Node. Le dire
    // explicitement vaut mieux qu'un « identique » qui ne couvre rien.
    for (const feuilles of Object.values(releve.scenarios)) {
      for (const f of feuilles) expect(f.images).toBe(0);
    }
  });
});

describe('les dates du navigateur suivent le fuseau du poste', () => {
  /**
   * ⚠️ Ce test ne mesure pas une parité : il FIGE l'écart assumé.
   *
   * Le navigateur date par `Intl` dans le fuseau de la machine qui regarde
   * l'écran ; le hub écrit en UTC, fuseau mentionné. Le même relevé change donc
   * d'heure — et parfois de jour — selon le poste. C'est ce que Benjamin verra
   * changer sur les documents qu'il envoie.
   */
  const quand = new Date(1788242400 * 1000); // 2026-09-01 06:00:00 UTC
  const fmt = (tz: string) =>
    new Intl.DateTimeFormat('fr', { dateStyle: 'short', timeStyle: 'medium', timeZone: tz }).format(
      quand,
    );

  it('rend une heure différente à Paris et à Houston, sans jamais nommer le fuseau', () => {
    expect(fmt('UTC')).toBe('01/09/2026 06:00:00');
    expect(fmt('Europe/Paris')).toBe('01/09/2026 08:00:00');
    expect(fmt('America/Chicago')).toBe('01/09/2026 01:00:00');
    // Aucune des trois ne dit dans quel fuseau elle est écrite.
    for (const tz of ['UTC', 'Europe/Paris', 'America/Chicago']) {
      expect(fmt(tz)).not.toMatch(/UTC|GMT|[+-]\d{2}:\d{2}/);
    }
  });
});
