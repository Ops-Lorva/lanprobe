import { describe, expect, it } from 'vitest';

// ⚠️ `router.ts` lit `location.hash` au CHARGEMENT du module, pour la valeur
// initiale de son store. Les tests tournent en Node, sans DOM : on pose donc
// l'objet avant d'importer, puis on importe. Un `import` statique en tête de
// fichier serait hissé avant cette ligne et le module échouerait à charger.
Object.defineProperty(globalThis, 'location', {
  value: { hash: '' },
  writable: true,
  configurable: true,
});

const { SETTINGS_TABS, href, parse } = await import('./router');

describe('adresses du hub', () => {
  it('donne à « Mon compte » son adresse propre, hors des réglages', () => {
    // 🔴 « Mon compte » a quitté la bande d'onglets de Réglages pour la barre
    // de gauche (`07d69a9`), et son adresse a suivi le 02/10 : elle était
    // restée `#/settings/account` au nom de favoris que personne n'a, ce qui
    // faisait surligner « Réglages » sur un écran qui n'en est plus un. Une
    // route qui ment sur l'endroit où elle mène force un cas particulier dans
    // tout ce qui la lit — c'est ce cas particulier qu'on supprime ici.
    expect(SETTINGS_TABS).not.toContain('account');
    expect(parse('#/account')).toEqual({ name: 'account' });
    expect(href({ name: 'account' })).toBe('#/account');
  });

  it('redirige l’ancienne adresse de « Mon compte » plutôt que d’afficher du vide', () => {
    // ⚠️ Un onglet resté ouvert sur `#/settings/account`, ou rechargé après la
    // mise à jour du hub, ne doit pas tomber sur « Général » — ni sur une page
    // blanche. La redirection ne coûte rien.
    expect(parse('#/settings/account')).toEqual({ name: 'account' });
  });

  it('retombe sur « Général » pour un onglet inconnu, et sur le parc pour le reste', () => {
    expect(parse('#/settings/pigeon-voyageur')).toEqual({ name: 'settings', tab: 'general' });
    expect(parse('#/settings')).toEqual({ name: 'settings', tab: 'general' });
    expect(parse('#/')).toEqual({ name: 'fleet' });
    expect(parse('')).toEqual({ name: 'fleet' });
  });

  it('redirige les anciennes entrées de navigation plutôt que de les perdre', () => {
    // Une URL qui retombe en silence sur le parc laisse croire que l'écran a
    // disparu.
    expect(parse('#/notifications')).toEqual({ name: 'settings', tab: 'alerts' });
    expect(parse('#/accounts')).toEqual({ name: 'settings', tab: 'accounts' });
  });

  it('garde la fiche d’une sonde collable dans un ticket', () => {
    expect(parse('#/probes/0c1e%20f2')).toEqual({ name: 'probe', id: '0c1e f2' });
    expect(href({ name: 'probe', id: '0c1e f2' })).toBe('#/probes/0c1e%20f2');
  });
});
