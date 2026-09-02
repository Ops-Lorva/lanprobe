import { describe, expect, it } from 'vitest';
import type { Probe } from './api';
import { fleetTotals, matchesFleetFilter, siteSeverity, sortKey } from './fleet-filter';

const AT = 1_788_015_096_000; // heure courante, en millisecondes
const NOW = AT / 1000;

/**
 * Sonde réduite à ce dont ces règles ont besoin. `status` est volontairement
 * réglé à contre-emploi dans les cas ci-dessous : c'est le défaut qu'on
 * corrige, il faut qu'un retour à `p.status` casse un test.
 */
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
    created_at: NOW,
    ...over,
  } as Probe;
}

describe('matchesFleetFilter', () => {
  it('ne retient pas une sonde silencieuse sous le filtre « en ligne »', () => {
    // Le défaut visible à l'écran : le hub tolère trois battements manqués et
    // répond encore `online`, pendant que la ligne affiche « Silencieuse ».
    // Deux affirmations contradictoires sur le même écran.
    const p = probe({ last_seen: NOW - 300, status: 'online' });
    expect(matchesFleetFilter(p, 'online', AT)).toBe(false);
    expect(matchesFleetFilter(p, 'silent', AT)).toBe(true);
  });

  it('retient les vraiment fraîches sous « en ligne »', () => {
    expect(matchesFleetFilter(probe({ last_seen: NOW - 10 }), 'online', AT)).toBe(true);
  });

  it('classe hors ligne une sonde que le hub dit encore « sans nouvelles »', () => {
    const p = probe({ last_seen: NOW - 3 * 3600, status: 'stale' });
    expect(matchesFleetFilter(p, 'offline', AT)).toBe(true);
    expect(matchesFleetFilter(p, 'silent', AT)).toBe(false);
  });

  it('« toutes » ne filtre rien, « en tampon » ne regarde que le tampon', () => {
    const p = probe({ last_seen: NOW - 300, buffered_points: 12 });
    expect(matchesFleetFilter(p, 'all', AT)).toBe(true);
    expect(matchesFleetFilter(p, 'buffering', AT)).toBe(true);
    expect(matchesFleetFilter(probe({}), 'buffering', AT)).toBe(false);
  });
});

describe('fleetTotals', () => {
  it('compte sur la fraîcheur, pas sur le statut du hub', () => {
    // Le bandeau du parc annonçait « 2 en ligne » sur deux lignes dont une
    // seule se disait en ligne.
    const totals = fleetTotals(
      [
        probe({ last_seen: NOW - 10, status: 'online' }),
        probe({ last_seen: NOW - 300, status: 'online', buffered_points: 5 }),
        probe({ last_seen: null, status: 'offline' }),
      ],
      AT,
    );
    expect(totals).toEqual({ online: 1, silent: 1, offline: 1, buffered: 5 });
  });

  it('additionne les tampons même quand la sonde va bien', () => {
    const totals = fleetTotals(
      [probe({ buffered_points: 3 }), probe({ buffered_points: 4 })],
      AT,
    );
    expect(totals.buffered).toBe(7);
  });
});

describe('siteSeverity', () => {
  it('range les sites par ce qui va le plus mal', () => {
    const offline = [probe({ last_seen: null })];
    const silent = [probe({ last_seen: NOW - 300 })];
    const buffering = [probe({ buffered_points: 9 })];
    const healthy = [probe({})];
    expect(siteSeverity(offline, AT)).toBeLessThan(siteSeverity(silent, AT));
    expect(siteSeverity(silent, AT)).toBeLessThan(siteSeverity(buffering, AT));
    expect(siteSeverity(buffering, AT)).toBeLessThan(siteSeverity(healthy, AT));
  });

  it('un site sans sonde ne passe pas devant un site en panne', () => {
    expect(siteSeverity([], AT)).toBeGreaterThan(siteSeverity([probe({ last_seen: null })], AT));
  });
});

describe('sortKey', () => {
  it('remonte les hors ligne, puis les silencieuses', () => {
    // Même ordre que la sévérité de site : ce qui appelle une action d'abord.
    expect(sortKey(probe({ last_seen: null }), AT)).toBeLessThan(
      sortKey(probe({ last_seen: NOW - 300 }), AT),
    );
    expect(sortKey(probe({ last_seen: NOW - 300 }), AT)).toBeLessThan(
      sortKey(probe({ last_seen: NOW - 5 }), AT),
    );
  });

  it('ignore le statut du hub, qui mentirait sur la silencieuse', () => {
    const silent = probe({ last_seen: NOW - 300, status: 'online' });
    const fresh = probe({ last_seen: NOW - 5, status: 'online' });
    expect(sortKey(silent, AT)).toBeLessThan(sortKey(fresh, AT));
  });
});
