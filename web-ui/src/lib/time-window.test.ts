import { describe, expect, it } from 'vitest';
import {
  MAX_RANGE_DAYS,
  applyRealtime,
  clipToWindow,
  metricsRange,
  parseLocalDate,
  relativeMinutes,
  toMetricsWindow,
  validateAbsoluteWindow,
  windowDomain,
  type WindowChoice,
} from './time-window';

const NOW = Date.UTC(2026, 8, 2, 12, 0, 0); // 2 septembre 2026, 12 h UTC
const DAY = 86_400_000;

describe('validateAbsoluteWindow', () => {
  it('accepte une plage passée et raisonnable', () => {
    // Le besoin exact : « vérifier si y a 10 jours on a eu une coupure ».
    expect(validateAbsoluteWindow(NOW - 11 * DAY, NOW - 9 * DAY, NOW)).toBeNull();
  });

  it('refuse une plage plus large que 90 jours', () => {
    // ⚠️ À un ping par seconde, une plage plus large met InfluxDB à genoux, et
    // c'est l'interface qui aura l'air en panne. Le refus doit venir d'ici,
    // avant la requête, pas d'un délai d'attente illisible.
    expect(validateAbsoluteWindow(NOW - MAX_RANGE_DAYS * DAY, NOW, NOW)).toBeNull();
    expect(validateAbsoluteWindow(NOW - (MAX_RANGE_DAYS * DAY + 1), NOW, NOW)).toBe('too_wide');
  });

  it('refuse un début après la fin', () => {
    // Sans ce refus, le hub rend une fenêtre vide, donc 0 % de disponibilité —
    // qu'on lit comme une panne totale et non comme une saisie inversée.
    expect(validateAbsoluteWindow(NOW - DAY, NOW - 2 * DAY, NOW)).toBe('reversed');
    expect(validateAbsoluteWindow(NOW - DAY, NOW - DAY, NOW)).toBe('reversed');
  });

  it('refuse une plage entièrement dans le futur', () => {
    expect(validateAbsoluteWindow(NOW + DAY, NOW + 2 * DAY, NOW)).toBe('future');
  });

  it('accepte une fin qui dépasse maintenant : « jusqu’à aujourd’hui » se saisit ainsi', () => {
    // La borne de fin couvre le jour ENTIER. Refuser ici rendrait impossible
    // de demander « du 1er à aujourd'hui » avant minuit.
    expect(validateAbsoluteWindow(NOW - 2 * DAY, NOW + DAY, NOW)).toBeNull();
  });

  it('refuse une saisie incomplète ou illisible', () => {
    expect(validateAbsoluteWindow(Number.NaN, NOW, NOW)).toBe('incomplete');
    expect(validateAbsoluteWindow(NOW - DAY, Number.NaN, NOW)).toBe('incomplete');
  });
});

describe('toMetricsWindow', () => {
  it('passe une fenêtre glissante telle quelle', () => {
    expect(toMetricsWindow({ kind: 'relative', range: '-6h' }, NOW)).toEqual({ range: '-6h' });
  });

  it('passe des bornes absolues en SECONDES, comme le hub les attend', () => {
    // ⚠️ Le hub compte en secondes (`MetricsQuery.start`). Envoyer des
    // millisecondes donnerait une fenêtre en l'an 57 000, donc vide.
    const w = toMetricsWindow(
      { kind: 'absolute', startMs: NOW - 2 * DAY, stopMs: NOW - DAY },
      NOW,
    );
    expect(w).toEqual({
      start: Math.floor((NOW - 2 * DAY) / 1000),
      stop: Math.floor((NOW - DAY) / 1000),
    });
  });

  it('ramène une fin future à maintenant', () => {
    // « Jusqu'à aujourd'hui » se saisit avec la fin du jour : demander à Influx
    // des points à venir n'a pas de sens et fait mentir l'axe.
    const w = toMetricsWindow({ kind: 'absolute', startMs: NOW - DAY, stopMs: NOW + DAY }, NOW);
    expect(w.stop).toBe(Math.floor(NOW / 1000));
  });
});

describe('metricsRange', () => {
  it('rend la fenêtre glissante inchangée', () => {
    expect(metricsRange({ kind: 'relative', range: '-10m' }, NOW)).toBe('-10m');
  });

  it('remonte jusqu’au début pour une plage absolue', () => {
    // ⚠️ `GET /api/probes/{id}/metrics` n'accepte QUE `range` : il ignore
    // `start`/`stop` (crates/lanprobe-web/src/web.rs, `probe_metrics`). On
    // demande donc tout depuis le début de la plage, et on coupe la fin ici.
    // Sans ce détour, les graphiques resteraient vides sur toute plage
    // absolue, en silence.
    expect(metricsRange({ kind: 'absolute', startMs: NOW - 3 * DAY, stopMs: NOW - DAY }, NOW)).toBe(
      `-${3 * 86_400}s`,
    );
  });
});

describe('clipToWindow', () => {
  const pts = [
    { t: NOW - 5 * DAY, v: 1 },
    { t: NOW - 3 * DAY, v: 2 },
    { t: NOW - DAY, v: 3 },
  ];

  it('ne coupe rien sur une fenêtre glissante', () => {
    expect(clipToWindow(pts, { kind: 'relative', range: '-7d' })).toEqual(pts);
  });

  it('ne garde que les points de la plage demandée', () => {
    // Le corollaire du détour ci-dessus : sans cette coupe, une plage
    // « du 20 au 22 » afficherait aussi tout ce qui s'est passé depuis.
    const choice: WindowChoice = {
      kind: 'absolute',
      startMs: NOW - 4 * DAY,
      stopMs: NOW - 2 * DAY,
    };
    expect(clipToWindow(pts, choice)).toEqual([{ t: NOW - 3 * DAY, v: 2 }]);
  });
});

describe('windowDomain', () => {
  it('borne l’axe sur la fenêtre demandée, pas sur les points reçus', () => {
    // « 6 h » doit montrer six heures, sinon trente minutes de relevés
    // occupent toute la largeur et on croit lire six heures de données.
    expect(windowDomain({ kind: 'relative', range: '-6h' }, NOW)).toEqual({
      from: NOW - 6 * 3_600_000,
      to: NOW,
    });
  });

  it('borne l’axe sur les DEUX bornes d’une plage absolue', () => {
    const choice: WindowChoice = { kind: 'absolute', startMs: NOW - 3 * DAY, stopMs: NOW - DAY };
    expect(windowDomain(choice, NOW)).toEqual({ from: NOW - 3 * DAY, to: NOW - DAY });
  });
});

describe('relativeMinutes', () => {
  it('écrit une durée Flux que le hub sait lire', () => {
    expect(relativeMinutes(10)).toBe('-10m');
    expect(relativeMinutes(7)).toBe('-7m');
  });
});

describe('parseLocalDate', () => {
  it('rend NaN sur une saisie vide plutôt qu’une date d’aujourd’hui', () => {
    // ⚠️ `new Date('')` vaut « Invalid Date », mais `Date.parse` de certains
    // formats partiels retombe sur une date valide. Un champ vide qui se lit
    // comme « aujourd'hui » lancerait une requête que personne n'a demandée.
    expect(Number.isNaN(parseLocalDate('', 'start'))).toBe(true);
  });

  it('couvre le jour ENTIER pour la borne de fin', () => {
    // « du 1er au 31 » sans ça s'arrêterait à minuit le 31 au matin, et le
    // dernier jour manquerait sans que personne ne s'en aperçoive.
    const start = parseLocalDate('2026-08-31', 'start');
    const stop = parseLocalDate('2026-08-31', 'stop');
    expect(stop - start).toBe(86_400_000 - 1000);
  });
});

describe('applyRealtime', () => {
  const before: WindowChoice = { kind: 'relative', range: '-6h' };

  it('adopte la fenêtre du temps réel quand le mode s’allume', () => {
    const next = applyRealtime({ choice: before, restore: null }, { on: true, windowMin: 10 });
    expect(next.choice).toEqual({ kind: 'relative', range: '-10m' });
    expect(next.restore).toEqual(before);
  });

  it('revient à la fenêtre précédente quand le mode s’arrête', () => {
    const during = { choice: { kind: 'relative', range: '-10m' } as WindowChoice, restore: before };
    expect(applyRealtime(during, { on: false, windowMin: 10 })).toEqual({
      choice: before,
      restore: null,
    });
  });

  it('ne rebascule pas à chaque tour d’horloge pendant le mode', () => {
    // Le parc se relit toutes les 3 s. Sans mémoire de la bascule, un clic sur
    // « 24 h » pendant le mode serait annulé trois secondes plus tard : la
    // fenêtre serait imposée, pas proposée.
    const during: { choice: WindowChoice; restore: WindowChoice | null } = {
      choice: { kind: 'relative', range: '-24h' },
      restore: before,
    };
    expect(applyRealtime(during, { on: true, windowMin: 10 })).toEqual(during);
  });

  it('ne restaure rien si l’utilisateur a choisi lui-même pendant le mode', () => {
    // Un choix explicite gagne sur une proposition. `restore` vidé au clic,
    // l'arrêt du mode ne défait plus ce qu'on vient de demander.
    const during: { choice: WindowChoice; restore: WindowChoice | null } = {
      choice: { kind: 'relative', range: '-24h' },
      restore: null,
    };
    expect(applyRealtime(during, { on: false, windowMin: 10 })).toEqual(during);
  });

  it('ne touche à rien quand le mode est éteint et l’a toujours été', () => {
    const idle = { choice: before, restore: null };
    expect(applyRealtime(idle, { on: false, windowMin: 10 })).toEqual(idle);
  });
});
