/**
 * Quelle entrée de la barre latérale est surlignée, pour chaque route.
 *
 * Le défaut qui motive ce fichier : sur l'écran « Mon compte », c'était
 * « Réglages » qui était surligné. La cause n'était pas le surlignage mais
 * l'adresse — l'écran était resté sous `settings/` après son départ des
 * onglets. Elle est devenue `#/account`, et la règle redevient un simple
 * appariement de nom.
 *
 * ⚠️ Si quelqu'un doit un jour écrire un `if` par route ici, c'est le signe
 * qu'une route ne désigne pas l'écran qu'elle affiche. C'est la route qu'il
 * faut corriger, pas ce module.
 */

import { describe, expect, it } from 'vitest';
import { activeNavId } from './nav-active';

describe('activeNavId', () => {
  it('surligne « Réglages » à la racine des réglages', () => {
    expect(activeNavId({ name: 'settings', tab: 'general' })).toBe('settings');
  });

  it('surligne « Réglages » sur les autres onglets de réglages', () => {
    expect(activeNavId({ name: 'settings', tab: 'storage' })).toBe('settings');
    expect(activeNavId({ name: 'settings', tab: 'accounts' })).toBe('settings');
    expect(activeNavId({ name: 'settings', tab: 'portscan' })).toBe('settings');
  });

  it('surligne « Mon compte », et PAS « Réglages », sur son écran', () => {
    expect(activeNavId({ name: 'account' })).toBe('account');
  });

  it('surligne le parc et le journal sur leur propre écran', () => {
    expect(activeNavId({ name: 'fleet' })).toBe('fleet');
    expect(activeNavId({ name: 'audit' })).toBe('audit');
  });

  it('ne surligne rien sur la fiche d’une sonde', () => {
    // La fiche d'une sonde n'a pas d'entrée dans la barre : aucune n'est
    // surlignée, et c'est voulu — surligner « Parc » dirait qu'on est sur la
    // liste alors qu'on est sur une fiche.
    expect(activeNavId({ name: 'probe', id: '0c1e f2' })).toBeNull();
  });
});
