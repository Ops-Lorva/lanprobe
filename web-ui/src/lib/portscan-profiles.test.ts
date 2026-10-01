import { describe, expect, it } from 'vitest';
import type { PortscanProfile } from './api';
import { parsePorts, formatPorts, portScanArgs, profileLabel } from './portscan-profiles';

const profile = (over: Partial<PortscanProfile> = {}): PortscanProfile => ({
  profile_id: 'cams',
  name: 'Caméras',
  ports: [554],
  origin_probe: null,
  created_at: 1_000,
  updated_at: 1_000,
  deleted_at: null,
  rev: 5,
  ...over,
});

describe('parsePorts', () => {
  it('trie et dédoublonne ce qui est saisi', () => {
    // ⚠️ `infra` répétait 161 dans la liste en dur, ce qui faisait annoncer
    // « 10 ports » pour neuf. La saisie à la main refera exactement ça.
    expect(parsePorts('443, 80, 443')).toEqual([80, 443]);
  });

  it('accepte les séparateurs qu’on tape vraiment', () => {
    // On colle une liste d'un ticket, pas une liste normalisée.
    expect(parsePorts('22 23\n53;123\t161')).toEqual([22, 23, 53, 123, 161]);
  });

  it('jette ce qui n’est pas un port plutôt que de l’envoyer à la sonde', () => {
    // Un `0` ou un `70000` ne scanne rien : les laisser passer ferait partir
    // une commande que la sonde refusera, longtemps après le clic.
    expect(parsePorts('0, 80, 70000, http, -1')).toEqual([80]);
  });

  it('rend une liste vide pour une saisie vide', () => {
    // ⚠️ Et une liste vide veut dire « la sonde garde la sienne », jamais
    // « ne scanne rien ».
    expect(parsePorts('   ')).toEqual([]);
  });
});

describe('formatPorts', () => {
  it('rend la liste telle qu’on la relit et la recolle', () => {
    expect(formatPorts([80, 443])).toBe('80, 443');
  });
});

describe('portScanArgs', () => {
  it('n’envoie AUCUN champ de ports quand le profil n’en porte pas', () => {
    // 🔴 La règle qui coûte cher si on la rate : `ports: []` ferait un scan
    // COMPLET là où on croyait restreindre — la sonde traite une liste vide
    // comme une absence de restriction. Vide veut dire « la sonde garde la
    // sienne », et ça ne se dit qu'en N'ENVOYANT PAS le champ.
    expect(portScanArgs('10.0.0.1', profile({ ports: [] }))).toEqual({ ip: '10.0.0.1' });
  });

  it('envoie les ports du profil quand il en porte', () => {
    expect(portScanArgs('10.0.0.1', profile({ ports: [80, 443] }))).toEqual({
      ip: '10.0.0.1',
      ports: [80, 443],
    });
  });

  it('se comporte comme un profil vide quand aucun n’est sélectionné', () => {
    // Un hub dont la liste est vide — tous supprimés — doit continuer de
    // lancer un scan : la sonde a sa propre liste.
    expect(portScanArgs('10.0.0.1', undefined)).toEqual({ ip: '10.0.0.1' });
  });
});

describe('profileLabel', () => {
  const t = (key: string) => (key === 'probe.profile_web' ? 'Toile' : key);

  it('traduit les quatre profils d’origine', () => {
    // Ils étaient traduits quand ils vivaient dans le code : les passer en
    // base ne doit pas rendre le hub anglais pour qui l'avait en français.
    expect(profileLabel(profile({ profile_id: 'web', name: 'Web' }), t)).toBe('Toile');
  });

  it('affiche le nom saisi pour tous les autres', () => {
    // Celui-là a été nommé par quelqu'un : le traduire serait lui en inventer
    // un autre.
    expect(profileLabel(profile({ name: 'Caméras' }), t)).toBe('Caméras');
  });
});
