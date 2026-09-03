/**
 * Paquets de la sonde, servis par le hub (contrat § 24).
 *
 * Le besoin naît à l'enrôlement : le hub donne un code, et il fallait aller
 * chercher le binaire ailleurs. Le hub le récupère, le **vérifie**, le garde et
 * le sert lui-même.
 *
 * 🔴 **Aucun appel sortant ne conditionne l'enrôlement.** Ce module n'est
 * appelé qu'à l'ouverture de la fenêtre des paquets : le code d'enrôlement
 * s'affiche comme d'habitude sur un hub sans Internet, et l'indisponibilité des
 * paquets se dit ici, à l'endroit qui la concerne.
 *
 * ⚠️ **Aucun numéro de version en dur.** Il vient du hub, ou il est absent —
 * un numéro figé se périme et devient un mensonge.
 */
import { request } from './api';

/** Les plateformes distribuables, dans l'ordre où on les propose. */
export const PLATFORMS = ['macos', 'windows', 'debian', 'debian-headless'] as const;
export type PackagePlatform = (typeof PLATFORMS)[number];

export interface HubPackage {
  platform: PackagePlatform;
  /** Celle du fichier concerné — celui du cache, ou celui de la release. */
  version: string | null;
  asset: string | null;
  cached: boolean;
  size: number | null;
  /**
   * 🔴 La CI publie-t-elle un `.sig` pour ce fichier. `false` = le hub ne le
   * servira pas : distribuer un binaire non vérifié depuis son propre serveur
   * serait moins sûr que de renvoyer vers GitHub.
   */
  signed: boolean;
  /** Le fichier existe-t-il dans la dernière release publiée. */
  publishable: boolean;
}

export interface PackagesPayload {
  reachable: boolean;
  error: string | null;
  tag: string | null;
  version: string | null;
  notes_url: string | null;
  keep_versions: number;
  packages: HubPackage[];
}

/**
 * L'état d'une ligne, et il y en a cinq parce qu'ils appellent cinq conduites
 * différentes. Un « indisponible » unique ferait réessayer là où il n'y a rien
 * à réessayer.
 */
export type PackageState =
  /** Sur le volume du hub, vérifié : il se sert, même sans Internet. */
  | 'cached'
  /** À récupérer. Le hub sortira, vérifiera, puis le gardera. */
  | 'fetchable'
  /** Aucune signature publiée : le hub ne le distribuera pas. */
  | 'unsigned'
  /** Absent de la dernière release. */
  | 'absent'
  /** Rien en cache, et le hub ne joint pas la liste des versions. */
  | 'offline';

export interface PackageRow {
  pkg: HubPackage;
  state: PackageState;
  /**
   * Le système du navigateur. ⚠️ Une **suggestion**, jamais un filtre : on
   * installe souvent la sonde depuis la machine où l'on est, mais pas toujours
   * — le technicien peut être sur son portable et équiper une autre machine.
   */
  suggested: boolean;
}

const CONNUES = new Set<string>(PLATFORMS);

function estPlateforme(v: unknown): v is PackagePlatform {
  return typeof v === 'string' && CONNUES.has(v);
}

function texte(v: unknown): string | null {
  return typeof v === 'string' && v !== '' ? v : null;
}

/**
 * Lit la réponse de `GET /api/packages` sans jamais jeter.
 *
 * ⚠️ Un corps inattendu rend une liste vide plutôt qu'une exception : la
 * fenêtre affiche alors sa phrase, au lieu de rester en chargement sans rien à
 * lire.
 */
export function parsePackages(body: unknown): PackagesPayload {
  const o = (typeof body === 'object' && body !== null ? body : {}) as Record<string, unknown>;
  const brut = Array.isArray(o.packages) ? o.packages : [];
  return {
    reachable: o.reachable === true,
    error: texte(o.error),
    tag: texte(o.tag),
    version: texte(o.version),
    notes_url: texte(o.notes_url),
    keep_versions: typeof o.keep_versions === 'number' ? o.keep_versions : 0,
    packages: brut
      .filter((p): p is Record<string, unknown> => typeof p === 'object' && p !== null)
      .filter((p) => estPlateforme(p.platform))
      .map((p) => ({
        platform: p.platform as PackagePlatform,
        version: texte(p.version),
        asset: texte(p.asset),
        cached: p.cached === true,
        size: typeof p.size === 'number' ? p.size : null,
        signed: p.signed === true,
        publishable: p.publishable === true,
      })),
  };
}

/**
 * Le système du navigateur, ou `null`.
 *
 * ⚠️ **Jamais le paquet sans interface** : il s'installe sur un serveur, pas
 * sur la machine d'où l'on regarde cet écran.
 */
export function detectPlatform(userAgent: string): PackagePlatform | null {
  const ua = userAgent.toLowerCase();
  // Un iPhone ou un Android consulte le hub sans jamais y installer de sonde :
  // mieux vaut ne rien suggérer que suggérer faux.
  if (/iphone|ipad|android/.test(ua)) return null;
  if (ua.includes('mac os') || ua.includes('macintosh')) return 'macos';
  if (ua.includes('windows')) return 'windows';
  if (ua.includes('linux') || ua.includes('x11')) return 'debian';
  return null;
}

/** L'état de chaque paquet, dans l'ordre où le hub les a rendus. */
export function packageRows(
  payload: PackagesPayload,
  detected: PackagePlatform | null,
): PackageRow[] {
  return payload.packages.map((pkg) => ({
    pkg,
    // 🔴 Le cache passe avant tout le reste : un paquet vérifié et posé sur le
    // volume se sert, que GitHub réponde ou non.
    state: pkg.cached
      ? 'cached'
      : !payload.reachable
        ? 'offline'
        : !pkg.publishable
          ? 'absent'
          : !pkg.signed
            ? 'unsigned'
            : 'fetchable',
    suggested: pkg.platform === detected,
  }));
}

/** Les trois appels de la fenêtre — jamais un `fetch` de plus. */
export const packagesApi = {
  list: () => request<unknown>('/api/packages'),

  /**
   * ⚠️ Geste explicite et séparé du téléchargement : le hub sort, tire le
   * fichier et vérifie sa signature. Ça prend le temps que ça prend, et
   * l'écran le dit — un bouton muet pendant qu'un paquet de 17 Mo arrive se
   * lit comme une panne.
   */
  fetch: (platform: PackagePlatform) =>
    request<unknown>(`/api/packages/${encodeURIComponent(platform)}/fetch`, { method: 'POST' }),

  /** Servi par le hub depuis son cache, jamais redirigé vers GitHub. */
  fileUrl: (platform: PackagePlatform) =>
    `/api/packages/${encodeURIComponent(platform)}/file`,
};
