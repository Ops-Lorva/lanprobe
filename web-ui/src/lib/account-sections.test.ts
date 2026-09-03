import { describe, expect, it } from 'vitest';
import { accountSections, type AccountFacts, type AccountSectionId } from './account-sections';

/** Un compte tout neuf : rien d'activé, rien d'appairé, rien en panne. */
function facts(patch: Partial<AccountFacts> = {}): AccountFacts {
  return {
    identity: { username: 'benjamin', role: 'Administrateur' },
    totp: { enabled: false, pending: false, enrolling: false, failed: false },
    passkeys: { count: 0, failed: false },
    devices: { loaded: true, active: 0, revoked: 0, codeLive: false, failed: false },
    ...patch,
  };
}

function section(patch: Partial<AccountFacts>, id: AccountSectionId) {
  const found = accountSections(facts(patch)).find((s) => s.id === id);
  if (!found) throw new Error(`section « ${id} » absente`);
  return found;
}

describe('sections de « Mon compte »', () => {
  it('rend les quatre sections, dans l’ordre où on les cherche', () => {
    expect(accountSections(facts()).map((s) => s.id)).toEqual([
      'identity',
      'totp',
      'passkeys',
      'devices',
    ]);
  });

  // ⚠️ La règle qui décide de tout le reste : un accordéon muet oblige à tout
  // ouvrir, donc ne fait rien gagner sur la pile qu'il remplace.
  it('donne à CHAQUE section un résumé qui se lit fermée', () => {
    for (const s of accountSections(facts())) {
      expect(s.summary.key, `section ${s.id}`).not.toEqual('');
    }
  });

  describe('identité', () => {
    it('nomme le compte et son rôle', () => {
      const s = section({}, 'identity');
      expect(s.summary.key).toBe('account.sum_identity');
      expect(s.summary.values).toEqual({ user: 'benjamin', role: 'Administrateur' });
    });

    it('ne devient pas muette quand le hub n’a pas encore répondu', () => {
      const s = section({ identity: { username: null, role: null } }, 'identity');
      expect(s.summary.values).toEqual({ user: '—', role: '—' });
    });

    it('se replie : elle ne porte aucun signal', () => {
      expect(section({}, 'identity').pinned).toBe(false);
    });
  });

  describe('second facteur', () => {
    it('dit « activé » quand il l’est', () => {
      const s = section({ totp: { enabled: true, pending: false, enrolling: false, failed: false } }, 'totp');
      expect(s.summary.key).toBe('account.sum_totp_on');
      expect(s.pinned).toBe(false);
    });

    it('dit « désactivé », et se replie : c’est un état, pas une alerte', () => {
      const s = section({}, 'totp');
      expect(s.summary.key).toBe('account.sum_totp_off');
      expect(s.pinned).toBe(false);
    });

    // Un secret généré que personne n'a confirmé n'est PAS un second facteur :
    // la connexion ne le demandera pas. Replié, on croit être protégé.
    it('ne se replie pas sur un secret jamais confirmé', () => {
      const s = section({ totp: { enabled: false, pending: true, enrolling: false, failed: false } }, 'totp');
      expect(s.summary.key).toBe('account.sum_totp_pending');
      expect(s.pinned).toBe(true);
    });

    // Le QR et le champ de vérification sont à l'écran : les replier
    // abandonnerait l'inscription en cours sans le dire.
    it('ne se replie pas pendant l’inscription', () => {
      const s = section({ totp: { enabled: false, pending: false, enrolling: true, failed: false } }, 'totp');
      expect(s.pinned).toBe(true);
    });

    it('ne se replie pas sur une erreur', () => {
      const s = section({ totp: { enabled: true, pending: false, enrolling: false, failed: true } }, 'totp');
      expect(s.pinned).toBe(true);
    });
  });

  describe('clés d’accès', () => {
    it('les compte', () => {
      const s = section({ passkeys: { count: 2, failed: false } }, 'passkeys');
      expect(s.summary.key).toBe('account.sum_passkeys');
      expect(s.summary.values).toEqual({ n: 2 });
    });

    // 🔴 Une clé d'accès inattendue doit se voir SANS déplier : c'est un moyen
    // de connexion permanent, et personne ne déplie une section qu'il ne
    // soupçonne pas déjà.
    it('ne se replie pas dès qu’une clé existe', () => {
      expect(section({ passkeys: { count: 1, failed: false } }, 'passkeys').pinned).toBe(true);
    });

    it('se replie quand il n’y en a aucune', () => {
      const s = section({}, 'passkeys');
      expect(s.summary.values).toEqual({ n: 0 });
      expect(s.pinned).toBe(false);
    });

    it('ne se replie pas sur une erreur', () => {
      expect(section({ passkeys: { count: 0, failed: true } }, 'passkeys').pinned).toBe(true);
    });
  });

  describe('appareils appairés', () => {
    it('compte les actifs', () => {
      const s = section(
        { devices: { loaded: true, active: 2, revoked: 0, codeLive: false, failed: false } },
        'devices',
      );
      expect(s.summary.key).toBe('account.sum_devices');
      expect(s.summary.values).toEqual({ n: 2 });
    });

    it('dit aussi les révoqués, qui encombrent la liste sans y compter', () => {
      const s = section(
        { devices: { loaded: true, active: 1, revoked: 3, codeLive: false, failed: false } },
        'devices',
      );
      expect(s.summary.key).toBe('account.sum_devices_revoked');
      expect(s.summary.values).toEqual({ n: 1, revoked: 3 });
    });

    // 🔴 Le seul organe qui rende le modèle acceptable : l'identité d'un
    // appareil n'expire pas. Un téléphone qu'on ne reconnaît pas doit se voir
    // sans qu'on ait eu l'idée de déplier.
    it('ne se replie pas dès qu’un appareil est actif', () => {
      const s = section(
        { devices: { loaded: true, active: 1, revoked: 0, codeLive: false, failed: false } },
        'devices',
      );
      expect(s.pinned).toBe(true);
    });

    it('se replie quand il n’y a que des révoqués', () => {
      const s = section(
        { devices: { loaded: true, active: 0, revoked: 2, codeLive: false, failed: false } },
        'devices',
      );
      expect(s.pinned).toBe(false);
    });

    it('se replie sur un compte vierge', () => {
      const s = section({}, 'devices');
      expect(s.summary.key).toBe('account.sum_devices');
      expect(s.summary.values).toEqual({ n: 0 });
      expect(s.pinned).toBe(false);
    });

    // Un code vivant porte un compte à rebours et un QR : replié, on le
    // réessaie trois fois avant de comprendre qu'il a expiré.
    it('ne se replie pas tant qu’un code d’appairage court', () => {
      const s = section(
        { devices: { loaded: true, active: 0, revoked: 0, codeLive: true, failed: false } },
        'devices',
      );
      expect(s.pinned).toBe(true);
    });

    // Un état qu'on ne sait pas lire se DIT. Un « 0 appareil » affiché sur une
    // liste qui n'a pas pu être chargée est un mensonge tranquille.
    it('dit « indisponible » plutôt que zéro quand la liste a échoué', () => {
      const s = section(
        { devices: { loaded: true, active: 0, revoked: 0, codeLive: false, failed: true } },
        'devices',
      );
      expect(s.summary.key).toBe('account.sum_unavailable');
      expect(s.pinned).toBe(true);
    });

    it('dit qu’elle charge, et ne s’épingle pas pour autant', () => {
      const s = section(
        { devices: { loaded: false, active: 0, revoked: 0, codeLive: false, failed: false } },
        'devices',
      );
      expect(s.summary.key).toBe('account.sum_loading');
      expect(s.pinned).toBe(false);
    });
  });
});
