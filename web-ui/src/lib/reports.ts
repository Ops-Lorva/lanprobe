/**
 * Rapports produits par le HUB (contrat § 23).
 *
 * 🔴 **Le classeur ne naît plus dans le navigateur.** Le hub le produit — le
 * même document, pas un équivalent — parce qu'une app native n'a ni `exceljs`
 * ni `<canvas>`. Sans cette bascule on ne passe pas de deux générateurs à un,
 * on passe à trois, et trois générateurs finissent par remettre trois chiffres
 * différents au même client.
 *
 * ⚠️ **Le générateur du navigateur n'est pas retiré pour autant** : il reste le
 * repli quand le hub est plus ancien que la route (`hubLacksReports`). On
 * ajoute un chemin, on n'en enlève pas — et `sla-report.ts` porte de toute
 * façon les calculs des tableaux affichés à l'écran.
 *
 * 🔴 **Un téléchargement à moitié réussi se solde par un échec visible, jamais
 * par un fichier.** Un classeur tronqué s'ouvre très bien, avec moins de
 * lignes, et part chez un client. `integrityFault` est ce qui l'arrête.
 *
 * ⚠️ Ce module vit à part de `api.ts` parce que `api.ts` est partagé par tous
 * les chantiers. `request` en est la seule porte exportée ; le seul `fetch`
 * écrit ici est celui du fichier, qui n'est pas du JSON — et il traduit
 * lui-même les refus du hub en `ApiError`.
 */
import { ApiError, request, type MetricsWindow } from './api';
// La traduction est celle de `sla-report.ts` : le catalogue des rapports est le
// même des deux côtés, qu'ils naissent ici ou dans le hub.
import type { Translate } from './sla-report';
import { absoluteTime } from './time';

/** Les quatre états qu'écrit le hub (`ReportState`, `reports.rs`). */
export type ReportState = 'preparing' | 'ready' | 'failed' | 'purged';

/**
 * Une ligne de la table `reports`, telle que le suivi et l'historique la
 * rendent (`ReportRecord`, `crates/lanprobe-web/src/reports.rs`).
 *
 * ⚠️ `state` est typé large : un hub plus récent peut nommer un état que cet
 * écran ne connaît pas, et le figer ici ferait mentir le type sans rien
 * empêcher.
 */
export interface HubReport {
  report_id: string;
  site_id: string;
  site_name: string;
  requested_by: string;
  /** `null` = demandé depuis le hub web, pas « inconnu ». */
  device_id: string | null;
  probe_ids: string[];
  /** La langue figée à la demande — celle de l'opérateur. */
  locale: string;
  range_start: number;
  range_stop: number;
  state: ReportState | string;
  /** « sonde 2 sur 3 — Lyon ». Ce que l'écran affiche pendant la préparation. */
  step: string | null;
  /** La cause NOMMÉE, jamais « erreur » : elle dit s'il faut réessayer ou appeler. */
  error: string | null;
  file_name: string | null;
  file_size: number | null;
  file_sha256: string | null;
  requested_at: number;
  ready_at: number | null;
  /** Fixée à la préparation et ne bouge plus : une date annoncée est tenue. */
  expires_at: number | null;
  purged_at: number | null;
  downloads: number;
}

/**
 * Lit la réponse de `GET /api/sites/{id}/reports` sans jamais jeter.
 *
 * Le hub sert `{ "reports": [...] }` — lu dans `list_site_reports`, pas
 * supposé. La liste nue est tolérée par prudence : un corps inattendu doit
 * donner un historique vide, pas une carte bloquée en chargement.
 */
export function parseReports(body: unknown): HubReport[] {
  const enveloppe =
    typeof body === 'object' && body !== null ? (body as { reports?: unknown }).reports : null;
  const list = Array.isArray(body) ? body : Array.isArray(enveloppe) ? enveloppe : [];
  return list.filter(
    (r): r is HubReport =>
      typeof r === 'object' && r !== null && typeof (r as HubReport).report_id === 'string',
  );
}

/** Une ligne d'historique, et ce que l'écran en dit. */
export interface ReportRow {
  report: HubReport;
  /** Le seul cas où le bouton « Télécharger » agit. */
  downloadable: boolean;
  /**
   * Le fichier est parti à la purge — la ligne, elle, reste.
   *
   * 🔴 À ne pas confondre avec un échec : un rapport purgé a existé, quelqu'un
   * l'a eu, et l'écran propose « Redemander ». Afficher une erreur ferait
   * douter du hub.
   */
  purged: boolean;
  failed: boolean;
  preparing: boolean;
}

/**
 * Les lignes dans l'ordre où on les cherche : la dernière demande en tête.
 *
 * ⚠️ Trié sur `requested_at` et non sur `ready_at` : c'est la demande qu'on se
 * rappelle avoir faite, et un rapport en préparation n'a pas encore de date de
 * fin — il tomberait en bas de la liste au moment précis où on le guette.
 */
export function reportRows(reports: HubReport[]): ReportRow[] {
  return [...reports]
    .sort((a, b) => b.requested_at - a.requested_at)
    .map((report) => ({
      report,
      downloadable: report.state === 'ready',
      purged: report.state === 'purged',
      failed: report.state === 'failed',
      preparing: report.state === 'preparing',
    }));
}

/**
 * Vrai quand le hub ne connaît pas la route des rapports : l'écran retombe
 * alors sur le générateur du navigateur.
 *
 * 🔴 **Un `404` PORTEUR d'un message n'en est pas un.** `site_scope_guard` rend
 * `404 { "error": "site inconnu" }` pour un site qui n'existe pas **ou qui
 * appartient à un autre client** ; le prendre pour un hub trop ancien ferait
 * produire le classeur par le navigateur, c'est-à-dire livrerait par une autre
 * porte les données que la portée refuse. Une route absente, elle, sort d'axum
 * sans aucun corps.
 */
export function hubLacksReports(e: unknown): boolean {
  if (!(e instanceof ApiError)) return false;
  if (e.status === 501) return true;
  if (e.status !== 404) return false;
  const corps = e.body;
  return !(typeof corps === 'object' && corps !== null && 'error' in corps);
}

/**
 * La date de purge portée par le `410` du fichier.
 *
 * ⚠️ `410` et non `404` : la ligne existe, l'écran vient de l'afficher. Sans
 * cette date, il ne peut dire que « erreur », et un rapport purgé se lit alors
 * comme un hub cassé.
 */
export function purgedAt(e: unknown): number | null {
  if (!(e instanceof ApiError) || e.status !== 410) return null;
  const corps = e.body;
  if (typeof corps !== 'object' || corps === null) return null;
  const date = (corps as { purged_at?: unknown }).purged_at;
  return typeof date === 'number' ? date : null;
}

// ── Intégrité du fichier ───────────────────────────────────────────────────

/** Ce que le hub annonce sur le corps qu'il sert. */
export interface Announced {
  /** `Content-Length`. `null` si l'en-tête manque. */
  length: number | null;
  /** `X-LanProbe-Sha256`, la même que `file_sha256`. */
  sha256: string | null;
}

/**
 * Ce qui empêche d'enregistrer le fichier.
 *
 * ⚠️ `expected` / `received` sont dans la faute et non dans un message tout
 * fait : « attendu 248 013 o · reçu 91 220 o » permet de décider s'il faut
 * réessayer ou appeler, et la phrase se traduit dans le catalogue.
 */
export type IntegrityFault =
  | { kind: 'length'; expected: number; received: number }
  | { kind: 'digest'; expected: string; received: string }
  | { kind: 'unverifiable' };

/** Lit les deux en-têtes du contrat § 23 sur une réponse. */
export function readAnnounced(headers: Headers): Announced {
  const brut = headers.get('Content-Length');
  const length = brut != null && brut.trim() !== '' && Number.isFinite(Number(brut))
    ? Number(brut)
    : null;
  const sha256 = headers.get('X-LanProbe-Sha256');
  return { length, sha256: sha256 && sha256.trim() !== '' ? sha256.trim().toLowerCase() : null };
}

/**
 * `null` si les octets reçus sont ceux annoncés ; sinon la raison de les
 * refuser.
 *
 * 🔴 Un XLSX tronqué échoue *généralement* à l'ouverture — « généralement »
 * n'est pas une garantie, et un rapport incomplet remis à un client est
 * exactement la valeur plausible et fausse que ce produit refuse.
 *
 * ⚠️ **`crypto.subtle` n'existe QUE dans un contexte sécurisé.** Un hub
 * auto-hébergé servi en clair sur une IP de LAN est le cas courant : refuser
 * tout téléchargement faute d'empreinte rendrait la fonctionnalité inutilisable
 * là où elle sert le plus. La longueur, elle, se compare partout — et elle
 * attrape la coupure, qui est le cas réel. Reste hors de portée la corruption
 * silencieuse à longueur constante, qui suppose un volume abîmé.
 */
export async function integrityFault(
  announced: Announced,
  bytes: Uint8Array,
): Promise<IntegrityFault | null> {
  if (announced.length != null && announced.length !== bytes.length) {
    return { kind: 'length', expected: announced.length, received: bytes.length };
  }
  if (announced.sha256 != null) {
    const digest = await sha256Hex(bytes);
    if (digest === null) {
      // Pas de `crypto.subtle` : la longueur a déjà tranché si elle était là.
      return announced.length == null ? { kind: 'unverifiable' } : null;
    }
    if (digest !== announced.sha256) {
      return { kind: 'digest', expected: announced.sha256, received: digest };
    }
    return null;
  }
  return announced.length == null ? { kind: 'unverifiable' } : null;
}

/** L'empreinte hexadécimale des octets, ou `null` hors contexte sécurisé. */
async function sha256Hex(bytes: Uint8Array): Promise<string | null> {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) return null;
  const digest = await subtle.digest('SHA-256', bytes as unknown as ArrayBuffer);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
}

/**
 * Un nombre d'octets **exact**, groupé selon la langue : « 248 013 ».
 *
 * ⚠️ Pas `humanBytes` : « 242,2 Ko » des deux côtés d'un fichier amputé de
 * trois cents octets, et le message censé trancher n'apprend plus rien.
 * L'unité vient du catalogue, qui porte déjà la phrase.
 */
export function exactBytes(n: number, locale: string): string {
  return new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(n);
}

/** Un fichier refusé : l'erreur porte la faute, pour que l'écran la nomme. */
export class ReportIntegrityError extends Error {
  readonly name = 'ReportIntegrityError';
  constructor(readonly fault: IntegrityFault) {
    super(`classeur refusé : ${fault.kind}`);
  }
}

// ── Les quatre routes ──────────────────────────────────────────────────────

/** Ce qu'une demande porte (contrat § 23). */
export interface NewReport {
  /**
   * ⚠️ **Absent = toutes les sondes du site**, jamais une liste vide : le hub
   * refuse celle-ci en `400`, et les confondre produirait le classeur du site
   * entier là où l'écran affichait zéro case cochée.
   */
  probe_ids?: string[];
  /** Résolue en bornes exactes en amont : un rapport porte sur une période convenue. */
  window: MetricsWindow;
  /** La langue de l'opérateur, figée dans la ligne par le hub. */
  locale: string;
}

const enc = encodeURIComponent;

export const reportsApi = {
  /** `202` : le hub a pris la demande, il n'a pas encore le fichier. */
  requestReport: (siteId: string, body: NewReport) =>
    request<{ report_id: string; state: string }>(`/api/sites/${enc(siteId)}/reports`, {
      method: 'POST',
      body: JSON.stringify({
        // 🔴 **Absent et vide ne sont pas la même chose**, et la nuance vaut un
        // classeur : absent = toutes les sondes du site, vide = une demande
        // sans sonde, que le hub refuse. Remplacer l'un par l'autre ici
        // produirait le classeur du site entier là où l'écran affichait zéro
        // case cochée — et ce classeur part chez un client.
        ...(body.probe_ids !== undefined ? { probe_ids: body.probe_ids } : {}),
        ...(body.window.start != null && body.window.stop != null
          ? { start: body.window.start, stop: body.window.stop }
          : { range: body.window.range ?? '-24h' }),
        locale: body.locale,
      }),
    }),

  /**
   * L'historique du site.
   *
   * ⚠️ Route DE SITE : le site est dans le chemin, donc la garde de portée
   * s'applique avant le handler. Un `GET /api/reports?site=` aurait laissé ce
   * contrôle au handler — la forme dont le § 17 dit qu'elle finit oubliée.
   */
  listSiteReports: (siteId: string) => request<unknown>(`/api/sites/${enc(siteId)}/reports`),

  /** Le suivi : `state`, `step`, `error`, `file_size`, `expires_at`. */
  trackReport: (reportId: string) => request<HubReport>(`/api/reports/${enc(reportId)}`),
};

/** Cadence du suivi. Le hub ne prépare qu'un rapport à la fois pour tout le parc. */
export const REPORT_POLL_MS = 1500;

/** Le suivi a été abandonné — l'écran s'est fermé, il n'y a rien à afficher. */
export class ReportAborted extends Error {
  readonly name = 'ReportAborted';
  constructor() {
    super('suivi du rapport abandonné');
  }
}

export interface PollOptions {
  /** Appelé à chaque tour tant que le rapport se prépare : « sonde 2 sur 3 ». */
  onStep?: (report: HubReport) => void;
  /** Injectables pour les tests — l'horloge et le réseau ne sont pas la logique. */
  track?: (reportId: string) => Promise<HubReport>;
  sleep?: (ms: number) => Promise<void>;
  /** Écran fermé : on cesse d'interroger. */
  signal?: { aborted: boolean };
}

/**
 * Suit un rapport jusqu'à ce que le hub ait tranché, et rend sa ligne finale.
 *
 * ⚠️ **Aucun délai maximal côté écran.** Afficher « échoué » au bout de N
 * secondes alors que le hub prépare encore serait une valeur plausible et
 * fausse ; le hub, lui, clôt en échec ce qu'un redémarrage a interrompu, avec
 * sa cause nommée. L'état affiché vient donc toujours de lui.
 *
 * ⚠️ Un échec ne jette pas : `state: 'failed'` porte la cause, et l'écran doit
 * la lire telle quelle plutôt qu'afficher « erreur ».
 */
export async function pollReport(reportId: string, opts: PollOptions = {}): Promise<HubReport> {
  const track = opts.track ?? reportsApi.trackReport;
  const sleep =
    opts.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
  for (;;) {
    const record = await track(reportId);
    if (opts.signal?.aborted) throw new ReportAborted();
    if (record.state !== 'preparing') return record;
    opts.onStep?.(record);
    await sleep(REPORT_POLL_MS);
    if (opts.signal?.aborted) throw new ReportAborted();
  }
}

/** Le classeur reçu, une fois vérifié. */
export interface ReportFile {
  bytes: Uint8Array;
  /** Celui du `Content-Disposition`, ou celui de la ligne à défaut. */
  fileName: string;
}

/**
 * Télécharge le fichier et **refuse de rendre quoi que ce soit** s'il n'est pas
 * celui qu'on attendait.
 *
 * 🔴 C'est la seule porte : enregistrer le corps avant vérification produirait
 * exactement le classeur amputé que ce chantier existe pour empêcher.
 *
 * ⚠️ `request` ne convient pas ici — elle lit le corps en texte et le parse en
 * JSON, ce qui abîmerait des octets binaires. Les refus, eux, sont bien du
 * JSON, et sont retraduits en `ApiError` à l'identique : `410` purgé, `409`
 * pas de fichier, `404` hors portée.
 */
export async function fetchReportFile(reportId: string, fallbackName = ''): Promise<ReportFile> {
  let res: Response;
  try {
    res = await fetch(`/api/reports/${enc(reportId)}/file`, { credentials: 'same-origin' });
  } catch {
    throw new ApiError(0, 'network');
  }

  if (!res.ok) {
    const raw = await res.text();
    let body: unknown = null;
    if (raw) {
      try {
        body = JSON.parse(raw);
      } catch {
        body = raw;
      }
    }
    const msg =
      body && typeof body === 'object' && 'error' in body
        ? String((body as { error: unknown }).error)
        : typeof body === 'string' && body.trim()
          ? body.trim()
          : `HTTP ${res.status}`;
    throw new ApiError(res.status, msg, body);
  }

  const announced = readAnnounced(res.headers);
  const bytes = new Uint8Array(await res.arrayBuffer());
  const faute = await integrityFault(announced, bytes);
  if (faute) throw new ReportIntegrityError(faute);

  return { bytes, fileName: dispositionName(res.headers) || fallbackName || `${reportId}.xlsx` };
}

/** Le nom du `Content-Disposition`, quand le hub en pose un. */
function dispositionName(headers: Headers): string {
  const brut = headers.get('Content-Disposition') ?? '';
  const m = /filename="([^"]+)"/.exec(brut) ?? /filename=([^;]+)/.exec(brut);
  return m ? m[1].trim() : '';
}

/**
 * Ce que l'écran affiche quand un rapport n'a pas abouti.
 *
 * 🔴 **Jamais « erreur ».** Chacun de ces cas appelle un geste différent :
 * retélécharger, redemander, appeler. Une phrase générique les confond tous, et
 * la première conséquence est qu'on reclique trois fois sur le même bouton.
 *
 * ⚠️ Un abandon rend la chaîne vide : l'écran s'est fermé, il n'y a rien à
 * signaler. Un message rouge après une fermeture volontaire fait chercher une
 * panne qui n'existe pas.
 */
export function reportErrorMessage(e: unknown, t: Translate, locale: string): string {
  if (e instanceof ReportAborted) return '';
  if (e instanceof ReportIntegrityError) {
    switch (e.fault.kind) {
      case 'length':
        return t('report.hub_truncated', {
          expected: exactBytes(e.fault.expected, locale),
          received: exactBytes(e.fault.received, locale),
        });
      case 'digest':
        return t('report.hub_corrupted');
      default:
        return t('report.hub_unverifiable');
    }
  }
  const purge = purgedAt(e);
  if (purge != null) return t('report.hub_purged_hint', { date: absoluteTime(purge, locale) });
  if (e instanceof Error) return e.message;
  return String(e);
}

/**
 * Remet le classeur au navigateur.
 *
 * ⚠️ Appelée seulement après `fetchReportFile`, donc après la vérification :
 * `Blob` et `<a download>` n'existent pas ailleurs, et c'est aussi pourquoi le
 * téléchargement et le contrôle sont deux fonctions — celui-ci se teste, pas
 * celle-là.
 */
export function saveReportFile(file: ReportFile): void {
  const blob = new Blob([file.bytes as unknown as BlobPart], {
    type: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = file.fileName;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}
