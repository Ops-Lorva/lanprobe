import { describe, expect, it } from 'vitest';
import { commandOrigin } from './command-origin';

/**
 * D'où vient une commande (contrat § 26).
 *
 * 🔴 Ce qui se teste ici n'est pas cosmétique. Une commande part de
 * l'INTÉRIEUR du réseau d'un client : un scan de ports, une découverte, un
 * test de débit. Devant une commande qu'on n'a pas lancée, la première
 * question est « qui », la seconde est « depuis où ». Le nom du compte ne
 * répond qu'à la première : le même compte sert au navigateur du bureau et à
 * l'app sur le téléphone du technicien.
 *
 * ⚠️ Et le pire des affichages serait une origine INVENTÉE. Une commande
 * empilée avant le § 26 n'en porte aucune ; la rendre « hub » affirmerait une
 * provenance que personne ne connaît, sur la seule trace qui serve à répondre
 * des mois après.
 */

describe('commandOrigin', () => {
  it('nomme le hub quand la commande vient du navigateur', () => {
    expect(commandOrigin({ origin: 'hub' })).toEqual({ kind: 'hub', label: null });
  });

  it('nomme l’appareil quand la commande vient d’un téléphone', () => {
    expect(
      commandOrigin({
        origin: 'device',
        origin_device_id: 'd-1',
        origin_device_name: 'iPhone de Claire',
      }),
    ).toEqual({ kind: 'device', label: 'iPhone de Claire' });
  });

  it('retombe sur l’identifiant quand le nom d’alors manque', () => {
    // ⚠️ Pas « hub », pas « inconnue » : on sait que ça vient d'un appareil,
    // et lequel. Seul son nom manque — l'identifiant reste exploitable pour
    // le retrouver dans l'écran « Appareils », même renommé ou révoqué.
    expect(
      commandOrigin({ origin: 'device', origin_device_id: 'd-1', origin_device_name: null }),
    ).toEqual({ kind: 'device', label: 'd-1' });
    expect(
      commandOrigin({ origin: 'device', origin_device_id: 'd-1', origin_device_name: '   ' }),
    ).toEqual({ kind: 'device', label: 'd-1' });
  });

  it('dit « appareil sans nom » plutôt que rien quand tout manque', () => {
    expect(commandOrigin({ origin: 'device' })).toEqual({ kind: 'device', label: null });
  });

  it('dit « inconnue » pour une commande empilée avant le § 26', () => {
    // 🔴 Le cas qui compte : un hub en service, sa file déjà remplie, mis à
    // jour. Ces lignes-là n'ont pas d'origine et ne doivent PAS passer pour
    // des commandes du hub.
    expect(commandOrigin({})).toEqual({ kind: 'unknown', label: null });
    expect(commandOrigin({ origin: null })).toEqual({ kind: 'unknown', label: null });
    expect(commandOrigin({ origin: '' })).toEqual({ kind: 'unknown', label: null });
  });

  it('dit « inconnue » pour une origine qu’elle ne sait pas lire', () => {
    // Un hub plus récent que l'interface (un onglet resté ouvert pendant une
    // mise à jour) pourrait servir une valeur de plus. L'afficher telle quelle
    // mettrait un mot anglais brut dans le tableau ; la prendre pour « hub »
    // mentirait. Elle est donc inconnue, ce qui est exact.
    expect(commandOrigin({ origin: 'cron' })).toEqual({ kind: 'unknown', label: null });
  });

  it('ne lit jamais l’appareil d’une origine « hub »', () => {
    // Défense en profondeur : si une ligne portait les deux à cause d'un
    // défaut en amont, l'écran doit dire ce que la colonne `origin` affirme,
    // pas deviner à partir des colonnes voisines.
    expect(
      commandOrigin({ origin: 'hub', origin_device_id: 'd-1', origin_device_name: 'iPhone' }),
    ).toEqual({ kind: 'hub', label: null });
  });
});
