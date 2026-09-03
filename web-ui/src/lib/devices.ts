/**
 * Appairage d'un téléphone et appareils appairés (contrat § 22).
 *
 * 🔴 **L'identité d'appareil n'expire pas.** Tant qu'elle n'est pas révoquée,
 * l'app se reconnecte toujours seule — c'est ce qui est demandé : on ne tape
 * pas son mot de passe en haut d'une échelle. Le prix est entier : un
 * téléphone perdu garde l'accès jusqu'à révocation. Ni le temps, ni un
 * changement de mot de passe, ni un redémarrage du hub ne le coupent.
 *
 * L'écran « Appareils » est donc la seule chose qui rend ce modèle acceptable,
 * et ce module porte ce qui peut l'être hors du gabarit : un tri qui met en
 * tête l'appareil qu'on vient vérifier, une lecture de réponse qui ne perd
 * aucune ligne, et les trois appels — jamais un `fetch` de plus.
 *
 * ⚠️ Ce module vit à part de `api.ts` parce que `api.ts` est partagé par tous
 * les chantiers : un fichier que plusieurs tâches modifient est un fichier que
 * personne ne peut modifier tranquillement. `request` en est la seule porte
 * exportée, et recoder un `fetch` ici perdrait la traduction des refus du hub
 * en `ApiError` — donc le message que l'écran doit afficher.
 */
import { request } from './api';

/**
 * Durée de vie d'un code d'appairage, en secondes.
 *
 * ⚠️ La même que celle d'un code d'enrôlement, et pour les mêmes raisons. Pas
 * 24 h « pour être tranquille » : le confort d'une fois par appareil ne paie
 * pas une fenêtre d'attaque d'une journée. Doit rester égale à
 * `PAIR_CODE_TTL_SECS` de `crates/lanprobe-web/src/pairing.rs` — c'est le repli
 * quand le hub ne renvoie pas d'échéance.
 */
export const PAIR_CODE_TTL_SECS = 15 * 60;

/**
 * Un appareil appairé, tel que le hub le sert (`PairedDevice`, `pairing.rs`).
 *
 * ⚠️ Le secret n'y figure pas et n'y figurera jamais : il est haché en base et
 * rendu une seule fois, à l'appairage.
 */
export interface PairedDevice {
  device_id: string;
  username: string;
  name: string;
  platform: string | null;
  app_version: string | null;
  created_at: number;
  /**
   * ⚠️ Affichée telle quelle (« vu il y a 41 j »), jamais convertie en
   * verdict : il n'y a **aucune** révocation par inactivité. Une révocation
   * « non vu depuis N jours » est une expiration déguisée qui se déclenche au
   * retour de congés.
   */
  last_seen: number | null;
  /** Avec `last_seen`, le seul détecteur d'un secret copié : il ne tourne pas. */
  last_ip: string | null;
  revoked_at: number | null;
}

/** Un code d'appairage fraîchement émis (`PairCode`, `pairing.rs`). */
export interface PairCode {
  /** En clair : il se lit à l'écran et se recopie à la main. */
  code: string;
  /** Epoch en secondes. */
  expires_at: number;
  /**
   * QR rendu **par le hub**, comme celui du second facteur : le secret ne
   * traverse aucune dépendance de plus, et la page reste lisible sur un réseau
   * fermé, sans CDN.
   *
   * ⚠️ Optionnel : un hub qui ne le sert pas laisse l'écran sur le code en
   * clair, qui est de toute façon le chemin de secours obligatoire.
   */
  qr_svg?: string | null;
  /**
   * Empreinte du certificat du hub, à épingler par le téléphone.
   * `null` quand le hub sert en clair : il n'y a alors aucun certificat DU HUB
   * à épingler — celui que le téléphone verra est celui du reverse proxy.
   */
  cert_fingerprint?: string | null;
}

/** Une ligne de la liste : l'appareil, et ce que l'écran en dit. */
export interface DeviceRow {
  device: PairedDevice;
  /** La ligne reste après révocation — aucune route ne supprime un appareil. */
  revoked: boolean;
}

/**
 * Lit la réponse de `GET /api/me/devices` sans jamais jeter.
 *
 * ⚠️ Un corps inattendu rend une liste vide plutôt qu'une exception : l'écran
 * affiche alors sa phrase, au lieu de rester en chargement sans rien à lire.
 * Les deux enveloppes sont acceptées — ce hub sert déjà les deux formes
 * ailleurs, et la route rend `501` tant qu'elle n'est pas écrite.
 */
export function parseDevices(body: unknown): PairedDevice[] {
  const enveloppe =
    typeof body === 'object' && body !== null ? (body as { devices?: unknown }).devices : null;
  const list = Array.isArray(body) ? body : Array.isArray(enveloppe) ? enveloppe : [];
  return list.filter(
    (d): d is PairedDevice =>
      typeof d === 'object' && d !== null && typeof (d as PairedDevice).device_id === 'string',
  );
}

/** Dernier signe de vie : la dernière vue, ou l'appairage si rien n'a suivi. */
function lastActivity(d: PairedDevice): number {
  return d.last_seen ?? d.created_at;
}

/**
 * Les lignes dans l'ordre où on les cherche : les actifs d'abord, le plus
 * récemment vu en tête.
 *
 * Un appareil révoqué reste en base et doit rester lisible — le journal
 * d'audit le nomme — mais il n'est plus la question. Mêlé aux actifs, il fait
 * chercher dans une liste dont la moitié des lignes ne veut plus rien dire.
 */
export function deviceRows(devices: PairedDevice[]): DeviceRow[] {
  return devices
    .map((device) => ({ device, revoked: device.revoked_at != null }))
    .sort((a, b) => {
      if (a.revoked !== b.revoked) return a.revoked ? 1 : -1;
      return lastActivity(b.device) - lastActivity(a.device);
    });
}

/**
 * Paliers d'unités, du plus fin au plus grossier, avec leur facteur suivant.
 *
 * ⚠️ **On s'arrête au jour** : « vu il y a 41 j » est le fait sur lequel un
 * humain tranche, « vu il y a 1 mois » ne l'est plus. Et comme il n'existe
 * aucune révocation par inactivité, cette ligne est le seul endroit où
 * l'ancienneté se lit — l'arrondir, c'est la perdre.
 */
const PALIERS: [Intl.NumberFormatOptions['unit'], number][] = [
  ['second', 60],
  ['minute', 60],
  ['hour', 24],
  ['day', Number.POSITIVE_INFINITY],
];

/**
 * Âge **nu** d'un horodatage : « 41 j », « 3 h », « 30 s ».
 *
 * ⚠️ Nu, et pas « il y a 41 j » : le catalogue porte déjà la phrase — « vu il
 * y a {age} ». Y glisser un `Intl.RelativeTimeFormat` afficherait « vu il y a
 * il y a 41 j ».
 *
 * ⚠️ Jamais négatif. Une horloge de téléphone en avance sur celle du hub est
 * banale ; « vu il y a -3 min » se lit comme une panne de l'écran.
 */
export function ageLabel(seconds: number, locale: string, atMs: number): string {
  let delta = Math.max(0, atMs / 1000 - seconds);
  for (const [unit, pas] of PALIERS) {
    if (delta < pas || pas === Number.POSITIVE_INFINITY) {
      return new Intl.NumberFormat(locale, {
        style: 'unit',
        unit,
        unitDisplay: 'short',
        maximumFractionDigits: 0,
      }).format(delta);
    }
    delta /= pas;
  }
  return '—';
}

/**
 * L'adresse à afficher à côté du code, pour la saisie à la main.
 *
 * ⚠️ L'adresse publique réglée sur le hub prime sur celle de la page :
 * `location.origin` peut être une IP interne joignable depuis ce poste et de
 * nulle part ailleurs — le téléphone recopierait alors une adresse qui ne
 * répond pas, et l'appairage échouerait sans rien dire de la raison.
 */
export function pairingAddress(hubPublicUrl: string | null | undefined, origin: string): string {
  const reglee = (hubPublicUrl ?? '').trim();
  // La barre finale se recopie à la main, caractère par caractère : elle ne
  // sert à rien et se saisit de travers.
  return (reglee || origin).replace(/\/+$/, '');
}

/**
 * Les trois appels de l'écran. Ceux de `/api/me/*` : chacun agit sur SON
 * compte, quel que soit son rôle — celui qui perd son téléphone à 22 h doit
 * pouvoir le révoquer seul, sans réveiller un administrateur.
 */
export const devicesApi = {
  /** Un code court, 15 minutes, usage unique. On en réémet un, c'est gratuit. */
  createPairCode: () => request<PairCode>('/api/me/pair-codes', { method: 'POST' }),

  myDevices: () => request<unknown>('/api/me/devices'),

  /**
   * ⚠️ `POST …/revoke`, jamais `DELETE`. Aucune route ne supprime un appareil :
   * la ligne reste, parce que le journal d'audit la nomme et qu'une ligne
   * d'audit qui pointe une cible disparue ne prouve rien.
   */
  revokeMyDevice: (deviceId: string) =>
    request<unknown>(`/api/me/devices/${encodeURIComponent(deviceId)}/revoke`, { method: 'POST' }),
};
