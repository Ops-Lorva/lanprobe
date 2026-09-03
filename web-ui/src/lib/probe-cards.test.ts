import { describe, expect, it } from 'vitest';
import type { MonitorState, SpeedtestRow } from './api';
import { engineLabel } from './charts';
import { monitorCards, speedCards, type LatencyCurve, type SpeedCurve } from './probe-cards';

const curve = (target: string, values: number[] = [12]): LatencyCurve => ({
  key: `latency_ms · ${target || '?'}`,
  target,
  label: target || 'Latence',
  color: 'var(--lp-series-1)',
  points: values.map((v, i) => ({ t: 1_000 + i, v })),
});

const monitor = (ip: string, over: Partial<MonitorState> = {}): MonitorState => ({
  ip,
  alive: true,
  latency_ms: 3,
  min_ms: 1,
  avg_ms: 2,
  max_ms: 9,
  uptime_pct: 100,
  samples: 42,
  ...over,
});

describe('monitorCards', () => {
  it('donne sa carte à une cible surveillée qui n’a pas encore de relevé', () => {
    // ⚠️ Le cas d'usage de l'ajout : on tape une adresse, on valide, et la
    // sonde ne la pinguera qu'au prochain battement. Ne rendre que les
    // courbes ferait disparaître la cible qu'on vient d'ajouter — on la
    // croirait refusée, et on recommencerait.
    const [card] = monitorCards([monitor('10.0.0.1')], []);
    expect(card.target).toBe('10.0.0.1');
    expect(card.curve).toBeNull();
    expect(card.stale).toBe(false);
  });

  it('garde la carte d’une cible mesurée que la sonde n’annonce plus, en la marquant', () => {
    // L'autre moitié de la distinction : une cible retirée reste dans la
    // fenêtre tant que ses points y sont. La montrer sans la marquer ferait
    // affirmer une surveillance qui n'existe plus.
    const cards = monitorCards([monitor('10.0.0.1')], [curve('10.0.0.9')]);
    const gone = cards.find((c) => c.target === '10.0.0.9');
    expect(gone?.stale).toBe(true);
    expect(gone?.monitor).toBeNull();
    expect(gone?.curve?.points).toHaveLength(1);
  });

  it('fusionne l’annonce et les mesures d’une même cible en UNE carte', () => {
    const cards = monitorCards([monitor('10.0.0.1')], [curve('10.0.0.1')]);
    expect(cards).toHaveLength(1);
    expect(cards[0].monitor?.samples).toBe(42);
    expect(cards[0].curve?.points).toHaveLength(1);
  });

  it('ne marque rien comme retiré quand la sonde n’annonce aucune surveillance', () => {
    // Une sonde antérieure à cette fonction n'annonce rien : conclure au
    // retrait de toutes ses cibles serait faux, et bruyant.
    const cards = monitorCards(null, [curve('10.0.0.9')]);
    expect(cards).toHaveLength(1);
    expect(cards[0].stale).toBe(false);
    expect(cards[0].monitor).toBeNull();
  });

  it('ne marque pas retirée une courbe qui ne nomme aucune cible', () => {
    // Sans adresse, rien ne permet de la rapprocher d'une surveillance
    // annoncée : on ne peut donc rien affirmer sur son retrait.
    const cards = monitorCards([monitor('10.0.0.1')], [curve('')]);
    expect(cards.find((c) => c.target === '')?.stale).toBe(false);
  });

  it('ouvre des clés distinctes, sinon deux cartes se recouvrent au rendu', () => {
    const cards = monitorCards([monitor('10.0.0.1'), monitor('10.0.0.2')], [curve('10.0.0.9')]);
    expect(new Set(cards.map((c) => c.key)).size).toBe(3);
  });

  it('marque RETIRÉE la cible que le hub dit retirée, jamais « plus annoncée »', () => {
    // 🔴 Le défaut rencontré à l'écran : la carte était construite depuis les
    // seules mesures de la fenêtre, donc une cible retirée restait sous
    // « Actives » avec un drapeau qui disait le contraire de l'onglet. Sur une
    // fenêtre de sept jours, elle y serait restée une semaine.
    const cards = monitorCards([monitor('10.0.0.1')], [curve('10.0.0.9')], ['10.0.0.9']);
    const gone = cards.find((c) => c.target === '10.0.0.9');
    expect(gone?.removed).toBe(true);
    // Les deux drapeaux s'excluent : « plus annoncée » est ce qu'on dit quand
    // on NE SAIT PAS pourquoi la cible a cessé. Ici on sait — c'est daté.
    expect(gone?.stale).toBe(false);
  });

  it('garde la courbe de la cible retirée : elle change de place, elle ne disparaît pas', () => {
    // ⚠️ On range l'historique au bon endroit, on ne le supprime pas : c'est
    // l'appelant qui rendra cette carte sous « Retirées ».
    const cards = monitorCards([], [curve('10.0.0.9', [12, 15, 11])], ['10.0.0.9']);
    const gone = cards.find((c) => c.target === '10.0.0.9');
    expect(gone?.curve?.points).toHaveLength(3);
  });

  it('n’invente pas un retrait pour une cible mesurée que rien ne réclame', () => {
    // ⚠️ Le cas de `8.8.8.8` : elle a des relevés dans la fenêtre, la sonde ne
    // l'annonce plus, et le hub n'a AUCUNE ligne pour elle — ni active, ni
    // retirée. La ranger dans « Retirées » affirmerait une décision que
    // personne n'a prise. Elle reste visible, marquée « plus annoncée ».
    const cards = monitorCards([monitor('10.0.0.1')], [curve('8.8.8.8')], ['10.0.0.9']);
    const orphan = cards.find((c) => c.target === '8.8.8.8');
    expect(orphan?.removed).toBe(false);
    expect(orphan?.stale).toBe(true);
  });

  it('sort des actives une cible retirée que la sonde annonce encore', () => {
    // Le retrait est un fait daté ; l'annonce est un état qui n'a pas encore
    // reçu l'ordre. `effectiveMonitors` écarte déjà ce cas en amont, mais la
    // règle ne doit pas dépendre de son appelant : deux endroits qui énoncent
    // le même fait finissent par diverger.
    const cards = monitorCards([monitor('10.0.0.9')], [curve('10.0.0.9')], ['10.0.0.9']);
    expect(cards.find((c) => c.target === '10.0.0.9')?.removed).toBe(true);
  });

  it('ne marque rien retiré quand l’appelant ne fournit aucune liste de retraits', () => {
    // Un hub antérieur à la liste partagée n'en tient pas : l'absence de
    // retrait connu n'est pas un retrait, et ne l'est pas non plus l'inverse.
    const cards = monitorCards([monitor('10.0.0.1')], [curve('10.0.0.9')]);
    expect(cards.every((c) => c.removed === false)).toBe(true);
  });
});

const speedCurve = (engine: string, field: string, values: number[]): SpeedCurve => {
  const direction = field === 'download_mbps' ? 'Descendant' : 'Montant';
  return {
    key: `${field}:${engine}`,
    engine,
    field,
    direction,
    // Sur le tableau de bord, les quatre courbes cohabitent : le libellé y
    // porte le moteur pour les distinguer.
    label: `${direction} · ${engineLabel(engine)}`,
    color: 'var(--lp-series-4)',
    points: values.map((v, i) => ({ t: i, v })),
  };
};

const run = (engine: string, started_at: number, over: Partial<SpeedtestRow> = {}): SpeedtestRow => ({
  started_at,
  engine,
  server_name: 'Serveur',
  download_mbps: 300,
  upload_mbps: 120,
  latency_ms: 8,
  jitter_ms: null,
  result_url: null,
  ...over,
});

describe('speedCards', () => {
  it('rassemble descendant et montant d’un moteur dans une seule carte', () => {
    const cards = speedCards(
      [
        speedCurve('ookla', 'download_mbps', [300]),
        speedCurve('ookla', 'upload_mbps', [120]),
      ],
      [],
    );
    expect(cards).toHaveLength(1);
    expect(cards[0].label).toBe('Speedtest.net');
    expect(cards[0].series).toHaveLength(2);
  });

  it('ne répète pas le moteur dans le libellé des séries de sa carte', () => {
    // La carte s'appelle déjà « Speedtest.net » : « Descendant · Speedtest.net »
    // juste en dessous écrit deux fois la même chose, ce que ce chantier
    // s'emploie précisément à retirer.
    const cards = speedCards(
      [
        speedCurve('ookla', 'download_mbps', [300]),
        speedCurve('iperf3', 'download_mbps', [940]),
      ],
      [],
    );
    expect(cards.map((c) => c.series[0].label)).toEqual(['Descendant', 'Descendant']);
  });

  it('sépare les moteurs, et eux seuls', () => {
    const cards = speedCards(
      [
        speedCurve('ookla', 'download_mbps', [300]),
        speedCurve('iperf3', 'download_mbps', [940]),
        speedCurve('iperf3', 'upload_mbps', [930]),
      ],
      [],
    );
    expect(cards.map((c) => c.label)).toEqual(['Speedtest.net', 'iperf3']);
    expect(cards[1].series).toHaveLength(2);
  });

  it('donne au descendant la même couleur d’une carte à l’autre', () => {
    // ⚠️ Les couleurs de `speedCurves` sont attribuées au rang, pour un
    // graphe unique où quatre courbes doivent se distinguer. Éclatées en
    // cartes, ce rang ferait que « descendant » change de couleur selon le
    // moteur — la seule chose que la couleur devait justement fixer.
    const cards = speedCards(
      [
        speedCurve('ookla', 'download_mbps', [300]),
        speedCurve('ookla', 'upload_mbps', [120]),
        speedCurve('iperf3', 'download_mbps', [940]),
        speedCurve('iperf3', 'upload_mbps', [930]),
      ],
      [],
    );
    const down = cards.map((c) => c.series.find((s) => s.key.startsWith('download'))?.color);
    const up = cards.map((c) => c.series.find((s) => s.key.startsWith('upload'))?.color);
    expect(new Set(down).size).toBe(1);
    expect(new Set(up).size).toBe(1);
    expect(down[0]).not.toBe(up[0]);
  });

  it('rattache à chaque carte le dernier test de SON moteur', () => {
    const cards = speedCards(
      [speedCurve('ookla', 'download_mbps', [300])],
      [run('iperf3', 900), run('ookla', 500), run('ookla', 800)],
    );
    expect(cards.find((c) => c.key === 'ookla')?.last?.started_at).toBe(800);
    expect(cards.find((c) => c.key === 'iperf3')?.last?.started_at).toBe(900);
  });

  it('donne sa carte à un moteur connu de l’historique seul', () => {
    // Un test plus ancien que la fenêtre affichée n'a pas de courbe : sans sa
    // carte, le moteur disparaîtrait de l'onglet alors qu'il a bien tourné.
    const cards = speedCards([], [run('iperf3', 100)]);
    expect(cards).toHaveLength(1);
    expect(cards[0].series).toEqual([]);
    expect(cards[0].last?.started_at).toBe(100);
  });

  it('ne rend aucune carte sans mesure ni historique', () => {
    // L'onglet garde alors son message d'absence : une grille vide ne dirait
    // rien de ce qui manque.
    expect(speedCards([], [])).toEqual([]);
  });
});
