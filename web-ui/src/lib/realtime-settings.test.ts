import { describe, expect, it, vi } from 'vitest';
import {
  MIN_HEARTBEAT_SECS,
  REALTIME_DEFAULTS,
  validateRealtime,
} from './realtime-settings';

describe('validateRealtime', () => {
  it('accepte les défauts du hub', () => {
    expect(validateRealtime(REALTIME_DEFAULTS)).toBeNull();
  });

  it('refuse une cadence sous le plancher que la sonde applique', () => {
    // ⚠️ La sonde fait `max(5)` de son côté. Un champ plus permissif
    // afficherait « 2 s » pendant qu'elle bat à 5 : une valeur plausible et
    // fausse, que personne ne remet en cause puisqu'elle vient du formulaire.
    expect(validateRealtime({ ...REALTIME_DEFAULTS, heartbeatSecs: 4 })).toBe('heartbeat');
    expect(validateRealtime({ ...REALTIME_DEFAULTS, heartbeatSecs: MIN_HEARTBEAT_SECS })).toBeNull();
  });

  it('refuse une cadence non entière ou vide', () => {
    expect(validateRealtime({ ...REALTIME_DEFAULTS, heartbeatSecs: Number.NaN })).toBe('heartbeat');
    expect(validateRealtime({ ...REALTIME_DEFAULTS, heartbeatSecs: 5.5 })).toBe('heartbeat');
  });

  it('refuse une durée nulle : le mode serait armé déjà expiré', () => {
    // Le bouton répondrait « c'est fait » sans que rien ne change sur la sonde.
    expect(validateRealtime({ ...REALTIME_DEFAULTS, durationMin: 0 })).toBe('duration');
    expect(validateRealtime({ ...REALTIME_DEFAULTS, durationMin: -1 })).toBe('duration');
  });

  it('refuse une fenêtre nulle : le graphe n’aurait aucun axe', () => {
    expect(validateRealtime({ ...REALTIME_DEFAULTS, windowMin: 0 })).toBe('window');
  });

  it('signale le premier champ fautif, pas tous', () => {
    // Le message nomme UN champ et l'écran y emmène. Trois erreurs à la fois
    // laisseraient chercher laquelle corriger en premier.
    expect(
      validateRealtime({ durationMin: 0, windowMin: 0, heartbeatSecs: 1 }),
    ).toBe('duration');
  });
});

describe('onglet « Temps réel » dans l’URL', () => {
  it('#/settings/realtime ouvre bien cet onglet', async () => {
    // L'onglet vit dans l'adresse : « ouvre #/settings/realtime » se dicte au
    // téléphone, « le quatrième onglet » non.
    //
    // ⚠️ Import dynamique après le postiche : `router.ts` lit `location.hash`
    // au chargement du module, et les tests tournent en environnement Node.
    // Un import statique casserait avant la première assertion.
    vi.stubGlobal('location', { hash: '' });
    vi.stubGlobal('addEventListener', () => {});
    const { parse } = await import('./router');
    expect(parse('#/settings/realtime')).toEqual({ name: 'settings', tab: 'realtime' });
  });
});
