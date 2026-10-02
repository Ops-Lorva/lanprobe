/**
 * Ce que le sélecteur de profils de la SONDE affiche (contrat § 25).
 *
 * ⚠️ Seule l'application de bureau consomme ces règles, mais elles vivent ici
 * parce que c'est ici que vitest tourne — voir l'en-tête du module. Une règle
 * d'affichage écrite dans un `.svelte` ne serait couverte par rien.
 */

import { describe, expect, it } from 'vitest';
import {
  isBuiltinProfile,
  visibleProfiles,
  activeProfile,
  type DisplayableProfile,
} from './portscan-profile-visibility';

const p = (over: Partial<DisplayableProfile> & { id: string }): DisplayableProfile => ({
  name: over.id,
  tcp_ports: [],
  udp_ports: [],
  ...over,
});

const base = p({ id: 'builtin:common', name: 'Common', tcp_ports: [22, 80], udp_ports: [53], builtin: true });
const sienne = p({ id: 'perso', name: 'Perso', tcp_ports: [22, 8006], udp_ports: [53, 123] });
const duHub = p({ id: 'web', name: 'Web', tcp_ports: [80, 443], from_hub: true });

describe('isBuiltinProfile', () => {
  it('reconnaît un profil de base par son drapeau comme par son identifiant', () => {
    // ⚠️ Les deux critères, pas un seul : l'interface pose `builtin: true`,
    // mais une liste écrite par une version antérieure peut n'avoir que le
    // préfixe. Se fier à un seul en laisserait passer la moitié.
    expect(isBuiltinProfile(base)).toBe(true);
    expect(isBuiltinProfile(p({ id: 'builtin:web', name: 'Web' }))).toBe(true);
    expect(isBuiltinProfile(sienne)).toBe(false);
    expect(isBuiltinProfile(duHub)).toBe(false);
  });
});

describe('visibleProfiles', () => {
  it('cache les profils de base dès que la sonde est rattachée à un hub', () => {
    // 🔴 La décision du 02/10 : « dès qu'une sonde passe en mode hub, elle
    // n'affiche plus du tout les profils de base qu'elle a en local ». Le hub
    // sème les mêmes et fait autorité — c'est ce qui fait disparaître les
    // doublons à la racine, sans jamais comparer des listes de ports.
    //
    // Les doublons constatés : `Common` local 16T/6U contre `Common` du hub
    // 0/0, `Web` 13T contre 8T, `Databases` 13T/1U contre 8T. Masquer sur le
    // NOM aurait caché des ports réellement scannés ; ne plus afficher les
    // profils de base n'en cache aucun, puisque le hub en sème l'équivalent.
    const visibles = visibleProfiles([base, sienne, duHub], true);
    expect(visibles.map(x => x.id)).toEqual(['perso', 'web']);
  });

  it('affiche tout quand la sonde est seule — c’est là que ses profils de base servent', () => {
    // Une sonde sans hub doit continuer de marcher SEULE : ses profils de base
    // sont son unique liste. C'est le seul moment où ils servent encore, et
    // c'est pourquoi on ne les supprime pas : ils resservent si elle est un
    // jour désenrôlée.
    const visibles = visibleProfiles([base, sienne, duHub], false);
    expect(visibles.map(x => x.id)).toEqual(['builtin:common', 'perso', 'web']);
  });

  it('ne montre qu’une fois un profil monté au hub puis redescendu', () => {
    // ⚠️ Le hub réutilise l'identifiant donné par la sonde : « Perso » revient
    // sur la même ligne, marqué `from_hub`. Il n'y a donc jamais deux entrées
    // — et c'est bien l'identifiant qui le garantit, pas le nom, que deux
    // profils différents peuvent partager.
    const redescendu = p({ id: 'perso', name: 'Perso', tcp_ports: [22, 8006], from_hub: true });
    expect(visibleProfiles([redescendu], true).map(x => x.id)).toEqual(['perso']);
  });

  it('ne garde rien quand une sonde rattachée n’a que ses profils de base', () => {
    // Cas réel d'une sonde fraîchement enrôlée dont le hub n'a encore rien
    // livré : la liste est vide, et l'écran doit le supporter — un scan sans
    // profil scanne la liste par défaut de la sonde.
    expect(visibleProfiles([base], true)).toEqual([]);
  });
});

describe('activeProfile', () => {
  it('retombe sur le premier visible quand le profil actif vient d’être caché', () => {
    // 🔴 Sans ce repli, une sonde qu'on vient d'enrôler garderait
    // `builtin:common` comme profil actif — devenu invisible — et le scan
    // partirait avec une liste de ports que l'écran n'affiche plus. Pire, le
    // sélecteur n'afficherait aucune sélection.
    expect(activeProfile([sienne, duHub], 'builtin:common')?.id).toBe('perso');
  });

  it('respecte le profil actif quand il est visible', () => {
    expect(activeProfile([sienne, duHub], 'web')?.id).toBe('web');
  });

  it('ne rend rien quand il n’y a plus aucun profil', () => {
    // ⚠️ `undefined`, et pas un profil vide fabriqué : c'est ce que l'appelant
    // traduit en « la sonde garde sa propre liste de ports ». Inventer ici un
    // profil à zéro port ferait scanner TOUT.
    expect(activeProfile([], 'perso')).toBeUndefined();
  });
});
