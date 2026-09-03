import { describe, expect, it } from 'vitest';
import {
  curveTarget,
  engineLabel,
  gapLimit,
  hasPreviousAnswer,
  internetSamples,
  normalizeState,
  segments,
  seriesColor,
} from './charts';

describe('normalizeState', () => {
  it('ramène les états écrits par la sonde à un vocabulaire fermé', () => {
    expect(normalizeState('online')).toBe('online');
    expect(normalizeState('up')).toBe('online');
    expect(normalizeState('OFFLINE')).toBe('offline');
    expect(normalizeState('limited')).toBe('limited');
  });

  it('rend « unknown » plutôt que de laisser passer une valeur imprévue', () => {
    // La bande d'état colorie ce qu'elle reçoit : une valeur inconnue qui
    // traverserait donnerait un segment sans couleur, donc invisible.
    expect(normalizeState(null)).toBe('unknown');
    expect(normalizeState('dégradé')).toBe('unknown');
    expect(normalizeState(42)).toBe('unknown');
  });
});

describe('curveTarget', () => {
  it('préfère l’étiquette `ip`, qui est la source sûre', () => {
    expect(
      curveTarget({ label: 'latency_ms · 10.0.30.12', tags: { ip: '10.0.30.12' } }),
    ).toBe('10.0.30.12');
  });

  it('retombe sur le libellé en lui retirant le nom du champ', () => {
    // ⚠️ Un graphe titré « latency_ms · 10.0.30.12 » afficherait un nom de
    // champ Influx à l'utilisateur ; comparé aux cibles annoncées par la
    // sonde, ce libellé brut ne correspondait jamais à rien.
    expect(curveTarget({ label: 'latency_ms · 10.0.30.12', tags: {} })).toBe('10.0.30.12');
  });

  it('rend une chaîne vide quand la courbe ne porte aucune cible', () => {
    // Une sonde qui ne surveille rien de nommé donne une courbe sans
    // étiquette : mieux vaut un titre générique que « latency_ms ».
    expect(curveTarget({ label: 'latency_ms', tags: {} })).toBe('');
  });
});

describe('engineLabel', () => {
  it('donne au moteur son nom de marque, pas son étiquette Influx', () => {
    // « ookla » est ce qu'écrit la sonde ; personne n'appelle ça autrement que
    // Speedtest.net, et c'est ce nom-là qui titre la carte de l'onglet Débit.
    expect(engineLabel('ookla')).toBe('Speedtest.net');
    expect(engineLabel('OOKLA')).toBe('Speedtest.net');
    expect(engineLabel('iperf3')).toBe('iperf3');
  });

  it('capitalise un moteur inconnu plutôt que de le masquer', () => {
    // Un moteur ajouté côté sonde avant l'interface doit rester lisible : le
    // taire ferait disparaître ses mesures de l'onglet.
    expect(engineLabel('bidule')).toBe('Bidule');
  });

  it('rend une chaîne vide quand la courbe ne nomme aucun moteur', () => {
    expect(engineLabel(undefined)).toBe('');
    expect(engineLabel('')).toBe('');
  });
});

describe('seriesColor', () => {
  it('donne une teinte distincte à chaque courbe d’un même graphique', () => {
    // Deux cibles pinguées, ou deux moteurs de speedtest, doivent se
    // distinguer : la même couleur pour deux courbes rend la légende inutile.
    const five = [0, 1, 2, 3, 4].map(seriesColor);
    expect(new Set(five).size).toBe(5);
  });

  it('boucle au-delà de la palette plutôt que de rendre une couleur vide', () => {
    expect(seriesColor(5)).toBe(seriesColor(0));
    expect(seriesColor(12)).toBe(seriesColor(2));
  });
});


describe('segments', () => {
  /** Cadence d'un ping : un relevé toutes les 2 s. */
  const every2s = (n: number, from = 1_788_000_000_000) =>
    Array.from({ length: n }, (_, i) => ({ t: from + i * 2_000, v: 20 }));

  it('ne coupe pas une série régulière', () => {
    expect(segments(every2s(20))).toHaveLength(1);
  });

  it('coupe sur une interruption : on a arrêté puis repris la surveillance', () => {
    // Le cas signalé : retirer un ping puis le remettre traçait une droite
    // parfaite au travers du trou — une latence affirmée pour une période où
    // personne ne mesurait.
    const before = every2s(10);
    const after = every2s(10, before[9].t + 20 * 60_000);
    const parts = segments([...before, ...after]);
    expect(parts).toHaveLength(2);
    expect(parts[0]).toHaveLength(10);
    expect(parts[1]).toHaveLength(10);
  });

  it('tolère la gigue sans couper à chaque hoquet', () => {
    // Un relevé qui arrive 3 s au lieu de 2 n'est pas une interruption ;
    // couper là hacherait la courbe en confettis.
    const pts = every2s(20);
    pts[10].t += 1_000;
    expect(segments(pts)).toHaveLength(1);
  });

  it('rend un seul segment quand il n’y a pas de cadence à observer', () => {
    // Deux points ne disent rien de la cadence : couper serait une invention.
    expect(gapLimit([{ t: 1 }, { t: 2 }])).toBe(Infinity);
    expect(segments([{ t: 1, v: 1 }, { t: 90_000_000, v: 2 }])).toHaveLength(1);
  });

  it('rend une liste vide sur une série vide', () => {
    expect(segments([])).toEqual([]);
  });
});

describe('internetSamples', () => {
  it('ramène les relevés du graphe en SECONDES', () => {
    // ⚠️ Le piège de tout ce chantier : les points du graphe sont en
    // millisecondes, les intervalles d'adresse publique en secondes. Sans
    // cette conversion aucun relevé ne tombe dans aucun intervalle et TOUT
    // ressort en « indéterminé » — sans la moindre erreur à l'écran.
    expect(internetSamples([{ t: 1_788_000_000_000, v: 'online' }])).toEqual([
      { timestamp: 1_788_000_000, alive: true, state: 'online' },
    ]);
  });

  it('ne compte « disponible » que « online », comme le hub', () => {
    // Le hub écrit `alive: state == "online"` dans le rapport SLA. Compter
    // « limited » comme disponible donnerait deux disponibilités pour la même
    // fenêtre : celle de l'écran et celle du classeur remis au client.
    const rows = internetSamples([
      { t: 0, v: 'online' },
      { t: 1_000, v: 'limited' },
      { t: 2_000, v: 'offline' },
      { t: 3_000, v: 'perdu' },
    ]);
    expect(rows.map((s) => s.alive)).toEqual([true, false, false, false]);
    expect(rows.map((s) => s.state)).toEqual(['online', 'limited', 'offline', 'unknown']);
  });
});

describe('hasPreviousAnswer', () => {
  it('dit non tant qu\u2019aucune lecture n\u2019a abouti', () => {
    // Le premier chargement, et lui seul, a le droit de vider l'écran : il n'y
    // a rien à garder.
    expect(hasPreviousAnswer(['loading', 'loading', 'loading'])).toBe(false);
  });

  it('dit oui dès qu\u2019un graphe porte des points', () => {
    expect(hasPreviousAnswer(['ready', 'loading', 'loading'])).toBe(true);
  });

  it('dit oui sur une fenêtre SANS mesure', () => {
    // 🔴 Le défaut : la règle demandait un graphe « ready ». Sur une fenêtre
    // vide les trois tonalités valent `empty`, elle répondait donc non, et le
    // tour de lecture remettait tout à `loading` — un chargeur qui revenait
    // toutes les trois secondes sur une carte qui n'a rien à montrer.
    expect(hasPreviousAnswer(['empty', 'empty', 'empty'])).toBe(true);
  });

  it('dit oui sur une lecture en échec', () => {
    // 🔴 Même défaut, et celui-là est pire : l'erreur disparaissait le temps
    // de chaque tour. Un message qui clignote se lit comme un incident qui va
    // et vient, alors que c'est la même panne qui dure.
    expect(hasPreviousAnswer(['error', 'error', 'error'])).toBe(true);
  });

  it('ne demande pas que TOUS aient répondu', () => {
    // Les trois graphes ne se remplissent pas ensemble : exiger l'unanimité
    // reviendrait à vider les deux autres à cause du troisième.
    expect(hasPreviousAnswer(['loading', 'empty', 'loading'])).toBe(true);
  });

  it('ne dit rien d\u2019une liste vide', () => {
    expect(hasPreviousAnswer([])).toBe(false);
  });
});
