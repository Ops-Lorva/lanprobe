import { beforeAll, describe, expect, it } from 'vitest';
import {
  buildSlaWorkbook,
  byPublicIp,
  coverageLabel,
  outages,
  stats,
  windowLabel,
  type PublicIpInterval,
  type Sample,
  type SlaPayload,
} from './sla-report';

const t = (k: string, v?: Record<string, string | number>) =>
  v ? `${k}:${Object.values(v).join(',')}` : k;

function series(pattern: string, from = 1_788_000_000): Sample[] {
  // 'o' = répond, 'x' = ne répond pas. Un relevé par seconde.
  return [...pattern].map((c, i) => ({
    timestamp: from + i,
    alive: c === 'o',
    latency_ms: c === 'o' ? 10 + i : null,
  }));
}

describe('outages', () => {
  it('isole chaque coupure, avec ce qu’elle a coûté', () => {
    const list = outages(series('ooxxoooxo'));
    expect(list).toHaveLength(2);
    expect(list[0].samples_lost).toBe(2);
    expect(list[1].samples_lost).toBe(1);
  });

  it('laisse une coupure en cours ouverte plutôt que d’inventer sa fin', () => {
    // ⚠️ La fermer sur l'instant de l'export fabriquerait une heure de
    // rétablissement qui n'a pas eu lieu — dans un document remis au client.
    const list = outages(series('ooxx'));
    expect(list).toHaveLength(1);
    expect(list[0].end).toBeNull();
  });

  it('ne voit aucune coupure sur une série saine', () => {
    expect(outages(series('oooo'))).toEqual([]);
    expect(outages([])).toEqual([]);
  });
});

describe('stats', () => {
  it('calcule la disponibilité sur « répond », pas sur la latence', () => {
    // Le piège : un hôte muet n'écrit pas de latence. Compter les latences
    // donnerait 100 % sur un hôte mort — l'inverse de la vérité.
    const dead: Sample[] = [
      { timestamp: 1, alive: false },
      { timestamp: 2, alive: false },
    ];
    expect(stats(dead).uptime_pct).toBe(0);
    expect(stats(dead).failed).toBe(2);
  });

  it('rend une disponibilité exacte sur une série mixte', () => {
    expect(stats(series('ooox')).uptime_pct).toBe(75);
  });

  it('ne prétend rien sur une série vide', () => {
    const s = stats([]);
    expect(s.total).toBe(0);
    expect(s.avg).toBeNull();
    expect(s.p95).toBeNull();
  });

  it('rend min, max et P95 depuis les latences observées', () => {
    const s = stats(series('ooooo'));
    expect(s.min).toBe(10);
    expect(s.max).toBe(14);
    expect(s.p95).not.toBeNull();
  });
});

describe('windowLabel', () => {
  it('écrit la période en toutes lettres', () => {
    // « 99,2 % » ne veut rien dire sans savoir sur quoi.
    expect(windowLabel('-24h', t)).toBe('sla.window_hours:24');
    expect(windowLabel('-7d', t)).toBe('sla.window_days:7');
  });

  it('laisse passer une fenêtre qu’elle ne sait pas nommer', () => {
    expect(windowLabel('-90m', t)).toBe('-90m');
  });
});

describe('windowLabel avec bornes explicites', () => {
  it('écrit « du … au … » plutôt qu’une durée relative', () => {
    // ⚠️ Un rapport contractuel porte sur une période convenue. « 7 derniers
    // jours » donnerait un chiffre différent à chaque ouverture du document.
    const out = windowLabel('1788000000..1788600000', t, 'fr');
    expect(out).toContain('sla.from');
    expect(out).toContain('sla.to');
    expect(out).not.toContain('window_days');
  });
});

const interval = (ip: string, from: number, to: number): PublicIpInterval => ({
  public_ip: ip,
  interface: 'en0',
  gateway: '10.6.8.1',
  local_subnet: '10.6.8.42',
  confirmed_from: from,
  confirmed_until: to,
  label: null,
});

const sample = (timestamp: number, state: string): Sample => ({
  timestamp,
  alive: state === 'online',
  state,
});

describe('byPublicIp', () => {
  it('impute une coupure suivie du retour sur la MÊME adresse à cette adresse', () => {
    // L'intervalle couvre la coupure : elle appartient bien à cette adresse.
    const rows = byPublicIp(
      [sample(10, 'online'), sample(20, 'offline'), sample(30, 'online')],
      [interval('88.120.0.1', 10, 30)],
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].public_ip).toBe('88.120.0.1');
    expect(rows[0].samples).toBe(3);
    expect(rows[0].uptime_pct).toBeCloseTo(66.67, 1);
  });

  it("n'impute à personne une coupure suivie d'une adresse DIFFÉRENTE", () => {
    // Le trou entre deux intervalles est la zone indéterminée : c'est tout
    // l'intérêt de fermer sur `confirmed_until`.
    const rows = byPublicIp(
      [sample(10, 'online'), sample(20, 'offline'), sample(30, 'online')],
      [interval('88.120.0.1', 10, 10), interval('172.58.0.9', 30, 30)],
    );
    const indetermine = rows.find((r) => r.public_ip === null);
    expect(indetermine?.samples).toBe(1);
    expect(rows.find((r) => r.public_ip === '88.120.0.1')?.uptime_pct).toBe(100);
    expect(rows.find((r) => r.public_ip === '172.58.0.9')?.uptime_pct).toBe(100);
  });

  it('range tout en indéterminé quand aucun intervalle ne couvre la fenêtre', () => {
    // Cas du lendemain de la migration : l'historique n'existe pas encore.
    const rows = byPublicIp([sample(10, 'online'), sample(20, 'online')], []);
    expect(rows).toHaveLength(1);
    expect(rows[0].public_ip).toBeNull();
    expect(rows[0].samples).toBe(2);
  });

  it('regroupe deux passages sur la même adresse en une seule ligne', () => {
    const rows = byPublicIp(
      [sample(10, 'online'), sample(50, 'online')],
      [
        interval('88.120.0.1', 10, 10),
        interval('172.58.0.9', 20, 30),
        interval('88.120.0.1', 50, 50),
      ],
    );
    expect(rows.filter((r) => r.public_ip === '88.120.0.1')).toHaveLength(1);
    expect(rows.find((r) => r.public_ip === '88.120.0.1')?.samples).toBe(2);
  });

  it('ne renvoie pas de ligne indéterminée quand il n’y a rien dedans', () => {
    const rows = byPublicIp([sample(10, 'online')], [interval('88.120.0.1', 0, 100)]);
    expect(rows.some((r) => r.public_ip === null)).toBe(false);
  });
});

function payloadWithTwoAddresses(): SlaPayload {
  return {
    probe: 'sonde-1',
    site: 'Site A',
    range: '-24h',
    generated_at: 1_788_000_000,
    targets: [],
    // La coupure de 20 tombe dans le trou entre les deux intervalles : elle
    // n'appartient à aucune des deux adresses.
    internet: [sample(10, 'online'), sample(20, 'offline'), sample(30, 'online')],
    speedtests: [],
    discovery: null,
    ports: null,
    public_ip_history: [interval('88.120.0.1', 10, 10), interval('172.58.0.9', 30, 30)],
  };
}

describe('buildSlaWorkbook', () => {
  // exceljs est importé à la demande et sa première résolution dépasse le
  // délai par défaut d'un test. On la sort des tests eux-mêmes : sinon le
  // premier d'entre eux échoue sur le temps de chargement, pas sur ce qu'il
  // vérifie — et l'ordre des tests déciderait lequel.
  beforeAll(async () => {
    await import('exceljs');
  }, 60_000);

  it('ajoute un onglet par adresse au classeur', async () => {
    const wb = await buildSlaWorkbook([payloadWithTwoAddresses()], t, 'fr');
    const sheet = wb.getWorksheet(t('report.ipSheet'));
    expect(sheet).toBeDefined();
    // en-tête + 2 adresses + la ligne indéterminée
    expect(sheet!.rowCount).toBeGreaterThanOrEqual(4);
  });

  it('écrit la ligne indéterminée en toutes lettres, pas en case vide', async () => {
    // ⚠️ Une case vide se lit comme une donnée manquante. C'est une part de la
    // période qu'on refuse d'imputer : elle doit se dire.
    const wb = await buildSlaWorkbook([payloadWithTwoAddresses()], t, 'fr');
    const sheet = wb.getWorksheet(t('report.ipSheet'))!;
    const addresses = sheet.getColumn(3).values.map((v) => String(v ?? ''));
    expect(addresses).toContain(t('report.ipUndetermined'));
  });

  it('ne fabrique aucun onglet quand la sonde n’a aucun relevé internet', async () => {
    const wb = await buildSlaWorkbook(
      [{ ...payloadWithTwoAddresses(), internet: [], public_ip_history: [] }],
      t,
      'fr',
    );
    expect(wb.getWorksheet(t('report.ipSheet'))).toBeUndefined();
  });
});

/**
 * L'onglet des ports ouverts : une charge où le LOT et les MACHINES n'ont pas
 * la même date, et où une machine n'en a aucune.
 *
 * `started_at` est volontairement très postérieur aux dates des machines : un
 * générateur qui daterait encore les lignes avec celle du lot se verrait
 * immédiatement, au lieu de passer parce que les deux se ressemblent.
 */
const T_LOT = 1_788_300_000; // le lot publié
const T_ROUTEUR = 1_788_000_000; // scanné bien avant
const T_NAS = 1_788_100_000;

function payloadPorts(): SlaPayload {
  return {
    probe: 'sonde-1',
    site: 'Site A',
    range: '-24h',
    generated_at: T_LOT,
    targets: [],
    internet: [],
    speedtests: [],
    discovery: null,
    ports: {
      started_at: T_LOT,
      cidr: null,
      hosts: [
        { ip: '192.168.1.1', scanned_at: T_ROUTEUR },
        { ip: '192.168.1.42', scanned_at: T_NAS },
        // Une machine d'avant la v28 du schéma, ou publiée par une sonde
        // antérieure : elle n'a pas de date, et ça doit se lire.
        { ip: '192.168.1.99' },
      ],
      ports: [
        { ip: '192.168.1.1', port: 443, proto: 'tcp', service: 'https' },
        { ip: '192.168.1.42', port: 22, proto: 'tcp', service: null },
        { ip: '192.168.1.99', port: 80, proto: 'tcp', service: 'http' },
      ],
    },
    public_ip_history: [],
  };
}

/**
 * Les cellules d'une colonne de l'onglet des ports, sans aucun en-tête.
 *
 * La ligne d'en-tête du tableau est repérée par son contenu et non par son
 * numéro : le bloc de tête du rapport compte une ligne de plus quand il n'y a
 * qu'une sonde, et un test qui compterait les lignes casserait pour une raison
 * sans rapport avec ce qu'il vérifie.
 */
async function colonnePorts(payload: SlaPayload, colonne: number): Promise<string[]> {
  const wb = await buildSlaWorkbook([payload], t, 'fr');
  const sheet = wb.getWorksheet(t('sla.sheet_ports'))!;
  let entete = 0;
  sheet.eachRow((row, i) => {
    if (String(row.getCell(2).value ?? '') === t('sla.col_scan_at')) entete = i;
  });
  const col: string[] = [];
  sheet.eachRow((row, i) => {
    if (i > entete) col.push(String(row.getCell(colonne).value ?? ''));
  });
  return col;
}

/** Les cellules de la colonne « Scan du » de l'onglet des ports. */
const colonneScanDuPorts = (payload: SlaPayload) => colonnePorts(payload, 2);

describe('onglet des ports — la date est celle de la MACHINE', () => {
  beforeAll(async () => {
    await import('exceljs');
  }, 60_000);

  it('date chaque ligne avec le scan de sa machine, pas avec le lot publié', async () => {
    // 🔴 La sonde republie TOUT son inventaire à chaque scan de ports : sans
    // quoi le hub, qui n'affichait que le dernier scan, perdait les machines
    // précédentes. `started_at` date donc le LOT. L'écrire sur une ligne de
    // port fait passer une machine scannée il y a trois semaines pour scannée
    // à l'instant — et c'est un classeur remis à un client.
    const col = await colonneScanDuPorts(payloadPorts());
    const attendu = (s: number) =>
      new Intl.DateTimeFormat('fr', { dateStyle: 'short', timeStyle: 'medium' }).format(
        new Date(s * 1000),
      );
    expect(col[0]).toBe(attendu(T_ROUTEUR));
    expect(col[1]).toBe(attendu(T_NAS));
    expect(col).not.toContain(attendu(T_LOT));
  });

  it('écrit « date inconnue » pour une machine sans date, jamais celle du lot', async () => {
    // ⚠️ Et jamais une case vide non plus : dans un classeur, une case vide se
    // lit comme un oubli d'export. Une absence de date est un fait, elle
    // s'écrit. Le libellé est celui de l'écran (`probe.scanned_unknown`) :
    // deux formulations pour le même manque laisseraient croire à deux cas.
    const col = await colonneScanDuPorts(payloadPorts());
    expect(col[2]).toBe(t('probe.scanned_unknown'));
  });
});

describe('stats — latences impossibles', () => {
  it('écarte une latence au-delà du délai sans toucher à la disponibilité', () => {
    // ⚠️ Le cas réel : un portable endormi pendant la mesure a publié
    // 1 045 504 ms — 17 minutes. Il écrasait la moyenne du rapport remis au
    // client. Mais l'hôte était bien vu vivant à ce moment-là : réécrire
    // `alive` changerait un SLA déjà communiqué, ce n'est pas la même décision.
    const s = stats([
      { timestamp: 10, alive: true, latency_ms: 10 },
      { timestamp: 20, alive: true, latency_ms: 30 },
      { timestamp: 30, alive: true, latency_ms: 1_045_504 },
    ]);
    expect(s.max).toBe(30);
    expect(s.avg).toBe(20);
    expect(s.uptime_pct).toBe(100);
  });

  it('garde une latence pile au délai', () => {
    const s = stats([{ timestamp: 10, alive: true, latency_ms: 1000 }]);
    expect(s.max).toBe(1000);
  });
});

describe('indéterminé — arbitrage du 02/09', () => {
  it('ne compte pas un relevé sans verdict dans le dénominateur', () => {
    // ⚠️ Une mesure sans `alive` n'est PAS une indisponibilité : la sonde n'a
    // rien dit. La compter comme un échec donnerait 50 % là où la seule mesure
    // existante dit 100 %.
    const s = stats([
      { timestamp: 10, alive: true, latency_ms: 10 },
      { timestamp: 20, alive: null, latency_ms: null },
    ]);
    expect(s.uptime_pct).toBe(100);
    expect(s.total).toBe(1);
    expect(s.undetermined).toBe(1);
  });

  it('ne rend AUCUN pourcentage quand rien n’a été mesuré', () => {
    // 🔴 Le cœur de l'arbitrage : `0` affirmait une panne totale, dans un
    // classeur remis au client. `null` ne s'imprime pas par accident.
    expect(stats([]).uptime_pct).toBeNull();
    expect(
      stats([{ timestamp: 10, alive: null, latency_ms: 12 }]).uptime_pct,
    ).toBeNull();
  });

  it('garde la latence d’un relevé indéterminé : c’est une mesure vraie', () => {
    const s = stats([{ timestamp: 10, alive: null, latency_ms: 12 }]);
    expect(s.min).toBe(12);
    expect(s.uptime_pct).toBeNull();
  });

  it('n’ouvre pas une coupure sur un relevé indéterminé', () => {
    // ⚠️ `!s.alive` est vrai pour `null` : sans garde explicite, chaque mesure
    // indéterminée serait comptée comme une panne — le faux 0 % revenu par la
    // porte de derrière, cette fois sous forme de coupures inventées.
    const coupures = outages([
      { timestamp: 10, alive: true, latency_ms: 10 },
      { timestamp: 20, alive: null, latency_ms: null },
      { timestamp: 30, alive: true, latency_ms: 11 },
    ]);
    expect(coupures).toEqual([]);
  });

  it('détecte toujours une vraie coupure', () => {
    const coupures = outages([
      { timestamp: 10, alive: true, latency_ms: 10 },
      { timestamp: 20, alive: false, latency_ms: null },
      { timestamp: 30, alive: true, latency_ms: 11 },
    ]);
    expect(coupures).toHaveLength(1);
    expect(coupures[0].samples_lost).toBe(1);
  });
});

describe('couverture — arbitrage B du 02/09', () => {
  it('ne dit rien quand la fenêtre est entièrement mesurée', () => {
    // ⚠️ Un « 0 % indéterminé » permanent est du bruit qui finit par masquer
    // le cas où ça compte. Complet = pas de mention.
    expect(coverageLabel({ window_secs: 600, covered_secs: 600, gap_secs: 0 })).toBeNull();
  });

  it('annonce la part mesurée quand il manque quelque chose', () => {
    // Le traducteur est passé comme en production : c'est lui qui interpole.
    const t = (k: string, v?: Record<string, string | number>) =>
      k === 'sla.coverage_partial' ? `${v?.pct} % de la période mesurée` : k;
    const l = coverageLabel({ window_secs: 1000, covered_secs: 874, gap_secs: 126 }, t);
    expect(l).not.toBeNull();
    // ⚠️ Ni vide ni numérique : la cellule ne doit pas pouvoir être moyennée.
    expect(Number.isNaN(Number(l))).toBe(true);
    expect(l).toContain('87');
  });

  it('ne déclare pas « complète » une fenêtre vide', () => {
    // Sans garde, `gap_secs === 0` rendait « tout va bien » sur une période
    // qui n'existe pas. Le même défaut a été corrigé côté Rust.
    expect(coverageLabel({ window_secs: 0, covered_secs: 0, gap_secs: 0 })).not.toBeNull();
  });

  it('ne prétend rien quand le hub n’a pas envoyé de couverture', () => {
    // Un hub antérieur à la fonctionnalité n'envoie pas le champ. Absence
    // d'information : on n'affiche rien, on n'invente pas « complet ».
    expect(coverageLabel(undefined)).toBeNull();
  });
});
