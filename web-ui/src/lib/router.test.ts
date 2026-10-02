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
  it('garde « Mon compte » adressable après son départ des onglets', () => {
    // 🔴 « Mon compte » a quitté la bande d'onglets de Réglages pour la barre
    // de gauche, mais SON ADRESSE N'A PAS CHANGÉ : elle est dans des favoris
    // et des captures d'écran. Elle doit donc rester dans `SETTINGS_TABS`,
    // qui sert de liste blanche à `parse` — l'en retirer « puisque ce n'est
    // plus un onglet » ferait silencieusement retomber l'entrée de la barre
    // de gauche sur « Général ».
    expect(SETTINGS_TABS).toContain('account');
    expect(parse('#/settings/account')).toEqual({ name: 'settings', tab: 'account' });
    expect(href({ name: 'settings', tab: 'account' })).toBe('#/settings/account');
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
