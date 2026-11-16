import { describe, expect, it } from 'vitest';
import type { Probe } from './api';
import { displayTotals, probeDisplay } from './probe-state';
import { matchesFleetFilter } from './fleet-filter';

const AT = 1_788_015_096_000;
const NOW = AT / 1000;

function probe(over: Partial<Probe>): Probe {
  return {
    probe_id: 'p',
    name: 'p',
    site_id: 's',
    site: 'Site',
    platform: null,
    version: null,
    last_seen: NOW,
    status: 'online',
    buffered_points: 0,
    created_at: NOW - 86_400,
    ...over,
  } as Probe;
}

describe('probeDisplay — l’état affiché d’une sonde, forme et mot', () => {
  it('dit « jamais vue » d’une sonde enrôlée qui n’a jamais battu, depuis son enrôlement', () => {
    // ⚠️ Pas « hors ligne » : on ne l'a jamais vue en ligne. La maquette lui
    // donne sa forme (anneau pointillé) et sa date d'enrôlement.
    const d = probeDisplay(probe({ last_seen: null, created_at: NOW - 3600 }), AT);
    expect(d).toEqual({ state: 'never', since: NOW - 3600, internet: null });
  });

  it('dit « dégradée » d’une sonde qui bat mais dont l’accès internet est limité', () => {
    // L'état « Dégradée » de la maquette se déduit des données existantes : la
    // sonde bat (fraîcheur en ligne) et `internet_state` vaut `limited`.
    const d = probeDisplay(
      probe({ internet_state: 'limited', internet_state_at: NOW - 2460 }),
      AT,
    );
    expect(d).toEqual({ state: 'degraded', since: NOW - 2460, internet: 'limited' });
  });

  it('dit aussi « dégradée » quand internet est coupé alors que la sonde bat', () => {
    const d = probeDisplay(probe({ internet_state: 'offline', internet_state_at: NOW - 60 }), AT);
    expect(d.state).toBe('degraded');
    expect(d.internet).toBe('offline');
  });

  it('ne dit pas « dégradée » d’une sonde muette : son état internet est figé', () => {
    // Sur une sonde qui ne bat plus, l'état internet est la dernière valeur
    // reçue. Il n'apprend rien de plus que le silence lui-même.
    const d = probeDisplay(
      probe({ last_seen: NOW - 600, internet_state: 'limited', internet_state_at: NOW - 900 }),
      AT,
    );
    expect(d).toEqual({ state: 'silent', since: NOW - 600, internet: null });
  });

  it('dit « hors ligne » depuis le dernier battement au-delà du seuil', () => {
    const d = probeDisplay(probe({ last_seen: NOW - 3 * 3600 }), AT);
    expect(d).toEqual({ state: 'offline', since: NOW - 3 * 3600, internet: null });
  });

  it('dit « en ligne » sans date quand tout va bien', () => {
    expect(probeDisplay(probe({ internet_state: 'online' }), AT)).toEqual({
      state: 'online',
      since: null,
      internet: null,
    });
  });
});

describe('displayTotals — le relevé chiffré du parc', () => {
  it('compte en ligne, à surveiller (dégradées + silencieuses), hors ligne, jamais vues', () => {
    const t = displayTotals(
      [
        probe({}),
        probe({ internet_state: 'limited' }),
        probe({ last_seen: NOW - 600 }),
        probe({ last_seen: NOW - 3 * 3600 }),
        probe({ last_seen: null }),
      ],
      AT,
    );
    expect(t).toEqual({ online: 1, degraded: 1, silent: 1, offline: 1, never: 1 });
  });
});

describe('matchesFleetFilter — filtres de la maquette', () => {
  it('« Problèmes » retient tout ce qui n’est pas en ligne, dégradées comprises', () => {
    expect(matchesFleetFilter(probe({}), 'problems', AT)).toBe(false);
    expect(matchesFleetFilter(probe({ internet_state: 'limited' }), 'problems', AT)).toBe(true);
    expect(matchesFleetFilter(probe({ last_seen: NOW - 600 }), 'problems', AT)).toBe(true);
    expect(matchesFleetFilter(probe({ last_seen: null }), 'problems', AT)).toBe(true);
  });

  it('« Temps réel » retient les sondes dont le mode tourne encore', () => {
    expect(matchesFleetFilter(probe({ realtime_until: NOW + 60 }), 'realtime', AT)).toBe(true);
    expect(matchesFleetFilter(probe({ realtime_until: NOW - 60 }), 'realtime', AT)).toBe(false);
    expect(matchesFleetFilter(probe({}), 'realtime', AT)).toBe(false);
  });

  it('« À surveiller » retient dégradées et silencieuses, « Jamais vues » les autres', () => {
    expect(matchesFleetFilter(probe({ internet_state: 'limited' }), 'attention', AT)).toBe(true);
    expect(matchesFleetFilter(probe({ last_seen: NOW - 600 }), 'attention', AT)).toBe(true);
    expect(matchesFleetFilter(probe({ last_seen: null }), 'attention', AT)).toBe(false);
    expect(matchesFleetFilter(probe({ last_seen: null }), 'never', AT)).toBe(true);
  });
});
