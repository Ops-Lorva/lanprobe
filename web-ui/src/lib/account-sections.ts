/**
 * Ce que chaque section de « Mon compte » dit **fermée**, et laquelle refuse
 * de se fermer.
 *
 * L'onglet a accumulé le mot de passe, le second facteur, les clés d'accès et
 * les appareils appairés : une pile sans hiérarchie qu'on parcourt en entier
 * pour y trouver une ligne. Le replier n'est utile qu'à deux conditions, et ce
 * module ne porte qu'elles.
 *
 * 🔴 **Ce qui alerte ne se replie pas.** Un appareil appairé qu'on ne
 * reconnaît pas, une clé d'accès inattendue : ce sont des moyens d'accès
 * permanents, et personne ne déplie une section qu'il ne soupçonne pas déjà.
 * Un accordéon qui cache un signal est pire que la pile qu'il remplace.
 *
 * ⚠️ **L'état de chaque section se lit fermé.** « 2 appareils », « second
 * facteur activé » : un accordéon muet oblige à tout ouvrir, donc ne fait rien
 * gagner.
 *
 * ⚠️ Ce module ne traduit pas : il rend des clés du catalogue et leurs
 * valeurs. Le rôle lui arrive déjà traduit — c'est l'écran qui sait dans
 * quelle langue il parle.
 */

export type AccountSectionId = 'identity' | 'totp' | 'passkeys' | 'devices';

/** Ce que la section affiche à côté de son titre, repliée comme dépliée. */
export interface SectionSummary {
  key: string;
  values?: Record<string, string | number>;
}

export interface AccountSection {
  id: AccountSectionId;
  summary: SectionSummary;
  /**
   * 🔴 Épinglée ouverte : la section porte un signal, et la refermer le
   * cacherait. Elle n'a alors pas de bouton de repli — grisé, il se cliquerait
   * trois fois avant qu'on comprenne.
   */
  pinned: boolean;
}

export interface AccountFacts {
  identity: {
    username: string | null;
    /** Libellé déjà traduit du rôle, tel que l'écran l'affiche. */
    role: string | null;
  };
  totp: {
    enabled: boolean;
    /** Un secret généré que personne n'a confirmé : la connexion ne le demande pas. */
    pending: boolean;
    /** Le QR est à l'écran, l'inscription est en cours. */
    enrolling: boolean;
    failed: boolean;
  };
  passkeys: { count: number; failed: boolean };
  devices: {
    loaded: boolean;
    active: number;
    revoked: number;
    /** Un code d'appairage court encore : son compte à rebours est à l'écran. */
    codeLive: boolean;
    failed: boolean;
  };
}

/** Un état qu'on ne sait pas lire se dit. Un « 0 » affiché à sa place ment. */
const UNAVAILABLE = 'account.sum_unavailable';

function identitySection(f: AccountFacts): AccountSection {
  return {
    id: 'identity',
    summary: {
      key: 'account.sum_identity',
      // Le tiret plutôt que le vide : une section sans résumé se lit comme une
      // section cassée, le temps que le hub réponde.
      values: { user: f.identity.username ?? '—', role: f.identity.role ?? '—' },
    },
    pinned: false,
  };
}

function totpSection(f: AccountFacts): AccountSection {
  const { enabled, pending, enrolling, failed } = f.totp;
  const key = enabled
    ? 'account.sum_totp_on'
    : pending || enrolling
      ? 'account.sum_totp_pending'
      : 'account.sum_totp_off';
  return {
    id: 'totp',
    summary: { key },
    // ⚠️ « Désactivé » n'épingle pas : c'est un état assumé, pas une alerte.
    // Une section qu'on ne peut plus fermer tant qu'on n'a pas activé la 2FA
    // serait un rappel permanent, et les rappels permanents ne se lisent plus.
    pinned: pending || enrolling || failed,
  };
}

function passkeysSection(f: AccountFacts): AccountSection {
  const { count, failed } = f.passkeys;
  return {
    id: 'passkeys',
    summary: { key: 'account.sum_passkeys', values: { n: count } },
    pinned: count > 0 || failed,
  };
}

function devicesSection(f: AccountFacts): AccountSection {
  const { loaded, active, revoked, codeLive, failed } = f.devices;
  if (failed) return { id: 'devices', summary: { key: UNAVAILABLE }, pinned: true };
  if (!loaded) return { id: 'devices', summary: { key: 'account.sum_loading' }, pinned: false };
  return {
    id: 'devices',
    summary:
      revoked > 0
        ? { key: 'account.sum_devices_revoked', values: { n: active, revoked } }
        : { key: 'account.sum_devices', values: { n: active } },
    // Un appareil révoqué n'épingle rien : il n'a plus accès. C'est l'actif —
    // celui dont l'identité n'expire pas — qui doit se voir sans déplier.
    pinned: active > 0 || codeLive,
  };
}

/** Les quatre sections, dans l'ordre où on les cherche. */
export function accountSections(f: AccountFacts): AccountSection[] {
  return [identitySection(f), totpSection(f), passkeysSection(f), devicesSection(f)];
}
