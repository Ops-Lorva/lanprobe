/**
 * Client HTTP du hub LanProbe.
 *
 * Seuls les endpoints décrits dans `docs/lanprobe-web-contrat.md` sont appelés :
 * aucune route n'est supposée au-delà du contrat.
 */

export type ProbeStatus = 'online' | 'stale' | 'offline';

export interface Site {
  site_id: string;
  name: string;
  probe_count: number;
  /** Retiré du parc sans être supprimé. `null` = actif. */
  archived_at?: number | null;
}

export interface Probe {
  probe_id: string;
  name: string;
  site_id: string;
  site: string;
  /** Retirée du parc sans être supprimée. `null` = visible. */
  archived_at?: number | null;
  platform: string | null;
  version: string | null;
  /** Epoch en secondes. `null` si la sonde n'a jamais battu. */
  last_seen: number | null;
  status: ProbeStatus;
  buffered_points: number;
  created_at: number;
  /** Identité réseau remontée par la sonde — contrat § 15. */
  public_ip?: string | null;
  public_ip_at?: number | null;
  interface?: string | null;
  local_ips?: string[];
  gateway?: string | null;
  /**
   * Verdict internet de la sonde. ⚠️ C'est le DERNIER connu, pas une mesure
   * fraîche : une sonde hors ligne garde le sien. L'interface doit donc le
   * présenter en fonction du statut, pas seul.
   */
  internet_state?: 'online' | 'limited' | 'offline' | null;
  internet_state_at?: number | null;
  /**
   * Surveillances ICMP **actives**, telles que la sonde les a annoncées.
   *
   * ⚠️ À ne pas confondre avec les cibles présentes dans les mesures : une
   * cible retirée garde ses points dans la fenêtre affichée, et se lirait
   * comme encore surveillée. Cette liste-ci dit ce qui tourne maintenant.
   */
  monitors?: MonitorState[] | null;
  monitors_at?: number | null;
  /**
   * Liste partagée tenue par le hub (table `probe_monitors`) : c'est elle qui
   * dit QUI est surveillé, et qui porte les suppressions datées.
   *
   * ⚠️ Elle ne remplace pas `monitors`, elle la complète : les mesures — répond,
   * latence, disponibilité — n'existent que dans l'annonce de la sonde. La
   * fusion des deux est dans `monitors.ts`, avec ses tests.
   *
   * Facultative : un hub antérieur à la liste partagée ne sert pas le champ.
   * Son absence ne veut donc PAS dire « plus rien n'est surveillé » — c'est
   * `monitors.ts` qui tient cette distinction, et l'écran retombe alors sur le
   * comportement d'avant.
   *
   * Attendu de `GET /api/probes`, sérialisé depuis `Db::monitors()` côté hub
   * (`MonitorEntry`) — la même liste que celle rendue au battement, retirées
   * comprises.
   */
  probe_monitors?: MonitorEntry[] | null;
  /**
   * Rotation de clé en attente de remise (contrat § 9). Champs facultatifs :
   * ils ne figurent pas encore dans l'exemple de `GET /api/probes`, l'interface
   * ne les affiche donc que si le hub les fournit.
   */
  token_rotation_pending?: boolean;
  token_version?: number;
  /**
   * Échéance du mode temps réel, epoch en secondes. `null` ou passée = cadence
   * normale.
   *
   * ⚠️ Aucune extinction n'est notifiée : le hub compare cette date à l'heure
   * courante à chaque battement, et l'interface fait exactement pareil. Il n'y
   * a donc rien à invalider, et une sonde ne peut pas rester affichée « en
   * temps réel » après l'échéance.
   */
  realtime_until?: number | null;
}

/** Code d'enrôlement court, à usage unique (contrat § 3). */
export interface EnrollCode {
  code: string;
  site: string;
  /** Renvoyé par le hub ; absent des exemples du contrat, d'où le `?`. */
  site_id?: string;
  /** Epoch en secondes. */
  expires_at: number;
}

/**
 * Enrôlement encore ouvert, tel que le hub le connaît.
 *
 * `probe_id` renseigné = ré-enrôlement d'une sonde existante ; `null` = place
 * réservée pour une sonde qui n'existe pas encore. Les deux ne sont pas la même
 * opération et ne se présentent pas pareil.
 *
 * ⚠️ `code` est **facultatif** : le hub l'efface dès qu'il est consommé ou
 * expiré, seul le haché subsiste. La liste ne devrait servir que des codes
 * ouverts, donc renseignés — mais l'interface traite l'absence plutôt que de
 * parier dessus.
 */
export interface PendingEnroll {
  site_id: string;
  site: string;
  probe_id: string | null;
  probe: string | null;
  created_at: number;
  expires_at: number;
  code?: string | null;
}

/**
 * Corps de `PUT /api/networks/label` (contrat § 15).
 *
 * ⚠️ La clé est le TRIPLET `(site_id, public_ip, gateway)`. Ni l'adresse seule
 * ni le couple `(adresse, passerelle)` ne suffisent : deux réseaux derrière le
 * même opérateur partagent l'IP publique, et sous CGNAT deux CLIENTS la
 * partagent aussi — avec, une fois sur deux, la même `192.168.1.1` de box.
 * Sans le site, « Maison de Durand » s'affichait dans le rapport de Martin.
 */
export interface NetworkLabel {
  /** Le site du réseau nommé. Le hub refuse en 404 s'il est hors portée. */
  site_id: string;
  public_ip: string;
  /** Chaîne vide quand la sonde n'a pas remonté de passerelle. */
  gateway: string;
  label: string;
}

/** Vue lecture seule d'InfluxDB (contrat § 8). */
export interface InfluxInfo {
  url: string;
  org: string;
  bucket: string;
  version: string | null;
  healthy: boolean | null;
  size_bytes: number | null;
  series_count: number | null;
}

export interface ReadToken {
  /** Identifiant de l'autorisation Influx — le hub le nomme `auth_id`. */
  auth_id: string;
  description: string;
  created_at: number;
}

/** D'où vient l'URL annoncée — cf. contrat § 7, ordre de résolution. */
export type AdvertiseSource = 'settings' | 'env' | 'host_header';

export interface AdvertiseTest {
  url: string;
  reachable: boolean;
  source: AdvertiseSource;
}

/** Table `settings` du hub (contrat § 7). Tout est réglable dans l'interface. */
export interface HubSettings {
  influx_url: string;
  influx_org: string;
  influx_bucket: string;
  /** `null` = laissé à la déduction du hub. */
  influx_advertise_url: string | null;
  /** Adresse publique du hub — le seul réglage d'adresse ; l'URL Influx en découle. */
  hub_public_url: string | null;
  /** 0 = rétention illimitée. */
  retention_days: number;
  heartbeat_interval_secs: number;
  /**
   * Délai avant qu'une sonde silencieuse déclenche une alerte. Réglé depuis
   * l'écran Notifications — c'est là qu'il veut dire quelque chose — mais il
   * vit dans `settings`, comme le reste, et part par `PUT /api/settings`.
   */
  notify_delay_secs: number;
  /**
   * Types d'alerte retenus (`probe.down`, `internet.down`…).
   *
   * ⚠️ Une liste **vide** est une valeur valide — « je ne veux rien
   * recevoir » — et non « non réglé ». Les confondre rendrait impossible de
   * tout couper sans démonter les canaux.
   */
  notify_events: string[];
  /** Sauvegarde planifiée. Coupée, plus aucune archive n'est produite seule. */
  backup_enabled: boolean;
  backup_interval_hours: number;
  /** Archives conservées. `0` = toutes ; baisser la valeur en supprime. */
  backup_keep_last: number;
  /**
   * Rétention de l'inventaire réseau (scans, machines, ports), en jours.
   * `0` = illimitée. ⚠️ Baisser la valeur SUPPRIME des scans — le hub exige
   * `confirm_data_loss`, comme pour la rétention des mesures.
   */
  inventory_days: number;
  /**
   * Le hub termine lui-même le TLS, avec un certificat auto-signé.
   * ⚠️ Lu au démarrage seulement : l'activer ne change rien tant que le
   * conteneur n'a pas redémarré.
   */
  tls_enabled: boolean;
  /**
   * Reverse proxies dont on accepte le `X-Forwarded-For`, en CIDR séparés par
   * des virgules.
   *
   * ⚠️ **Vide par défaut, et c'est le comportement sûr** : l'en-tête est alors
   * ignoré, on lit l'adresse de la socket. Le lire « s'il est là » laisserait
   * n'importe qui forger son adresse source.
   */
  trusted_proxies: string;
  /**
   * Durée du mode temps réel, en minutes, avant extinction automatique.
   *
   * ⚠️ La minuterie n'est pas un confort : on redescend de l'échelle, la
   * caméra marche, on passe à la suivante — personne ne revient éteindre le
   * mode. Sans elle, tout le parc finit par battre toutes les 5 s sans que
   * quiconque sache pourquoi.
   */
  realtime_duration_min: number;
  /**
   * Fenêtre affichée pendant le mode temps réel, en minutes.
   *
   * ⚠️ Une heure écrase l'instant : à un ping par seconde, ce qu'on est venu
   * regarder tient dans quelques pixels. L'historique, lui, est le travail du
   * rapport SLA — pas de l'écran qu'on regarde en intervenant.
   */
  realtime_window_min: number;
  /**
   * Cadence du battement pendant le mode temps réel, en secondes.
   *
   * ⚠️ Plancher de 5 s côté hub, parce que la sonde applique `max(5)`.
   * Accepter moins ici afficherait une cadence qu'elle n'honore pas.
   */
  realtime_heartbeat_secs: number;
}

export const SETTINGS_DEFAULTS: HubSettings = {
  influx_url: 'https://127.0.0.1:8086',
  influx_org: 'lanprobe',
  influx_bucket: 'lanprobe',
  influx_advertise_url: null,
  hub_public_url: null,
  retention_days: 0,
  heartbeat_interval_secs: 60,
  notify_delay_secs: 300,
  notify_events: ['probe.down', 'probe.up'],
  backup_enabled: true,
  backup_interval_hours: 24,
  backup_keep_last: 7,
  inventory_days: 0,
  tls_enabled: false,
  trusted_proxies: '',
  realtime_duration_min: 30,
  realtime_window_min: 10,
  realtime_heartbeat_secs: 5,
};

// ── Sauvegarde ──────────────────────────────────────────────────────────────

/**
 * Une archive du répertoire de sauvegarde. Le hub la décrit **d'après son
 * nom** : il n'ouvre pas les archives pour dresser la liste, une archive
 * abîmée doit y figurer quand même.
 */
export interface BackupArchive {
  file: string;
  path: string;
  bytes: number;
  hub_version: string;
  /** Epoch en secondes, déduit du nom du fichier. */
  created_at_unix: number;
  /** Même instant en RFC 3339. */
  created_at: string;
}

export interface BackupList {
  /** Chemin du volume, tel que le hub le voit. Il porte `secret.key`. */
  dir: string;
  enabled: boolean;
  interval_hours: number;
  keep_last: number;
  /** La plus récente en tête — c'est le hub qui trie. */
  backups: BackupArchive[];
}

/**
 * Verdict d'une sauvegarde immédiate.
 *
 * ⚠️ `influx_included: false` avec un `influx_error` n'est **pas** un échec :
 * l'archive existe et porte la base, les comptes et les secrets. Mais elle
 * n'a pas les mesures, et c'est le genre de détail qu'on découvre au pire
 * moment si l'écran l'a passé sous silence.
 */
export interface BackupCreated {
  file: string;
  bytes: number;
  created_at: string;
  hub_version: string;
  schema_version: number;
  influx_included: boolean;
  influx_error: string | null;
  /** Archives retirées par la rétention dans la foulée. */
  pruned: string[];
}

/**
 * Verdict d'une restauration.
 *
 * ⚠️ `restart_required` vaut **toujours** `true` : le processus tient encore
 * l'ancienne base ouverte et continuera de la servir. Une interface qui ne le
 * dit pas laisse conclure que la restauration n'a rien fait.
 */
export interface RestoreReport {
  ok: boolean;
  hub_version: string;
  schema_version: number;
  /** Date de fabrication de l'archive, RFC 3339. */
  created_at: string;
  /** Noms des fichiers remplacés dans le volume. */
  restored: string[];
  /** Où l'état précédent a été déplacé. `null` = le volume était vide. */
  moved_aside: string | null;
  influx_restored: boolean;
  influx_error: string | null;
  restart_required: boolean;
}

// ── Comptes (contrat § 11) ──────────────────────────────────────────────────

/** Ordonnés : `viewer < operator < admin`. */
export type Role = 'admin' | 'operator' | 'viewer';

/**
 * Qui est connecté, tel que le hub le voit — `GET /api/me`, derrière le garde
 * de session.
 *
 * Le rôle est relu par le hub à chaque requête (contrat § 11) : celui-ci est un
 * instantané, pas un droit acquis. L'interface s'en sert pour ne pas proposer
 * ce qui sera refusé — jamais pour décider si c'est permis.
 */
export interface Identity {
  username: string;
  role: Role;
}

/** Du plus puissant au moins puissant : c'est l'ordre dans lequel on choisit. */
export const ROLES: Role[] = ['admin', 'operator', 'viewer'];

/**
 * Un compte. **`disabled_at` remplace la suppression** : le hub n'a pas de
 * `DELETE /api/users/{…}` et n'en aura pas, parce que le journal d'audit nomme
 * ce compte et qu'une ligne d'audit qui désigne un compte disparu ne prouve
 * plus rien.
 */
export interface Account {
  username: string;
  role: Role;
  created_at: number;
  /** Epoch en secondes quand le compte est désactivé, `null` s'il est actif. */
  disabled_at: number | null;
  /**
   * Sites que ce compte a le droit de voir (contrat § 17).
   *
   * ⚠️ **Liste vide = accès à TOUS les sites**, pas à aucun. C'est le cas
   * courant et il ne coûte rien à configurer ; l'interpréter à l'envers
   * afficherait « ne voit rien » pour la quasi-totalité des comptes.
   */
  sites: string[];
}

// ── Journal d'audit (contrat § 11) ──────────────────────────────────────────

export type AuditOutcome = 'success' | 'failure';

export interface AuditEntry {
  id: number;
  at: number;
  /** `null` = tentative anonyme (connexion sur un compte inconnu). */
  actor: string | null;
  action: string;
  target: string | null;
  outcome: AuditOutcome;
  detail: string | null;
}

export interface AuditPage {
  entries: AuditEntry[];
  /** Curseur de la page suivante. `null` = fin du journal. */
  next_before_id: number | null;
}

/**
 * Vocabulaire fermé des actions, tel que le contrat § 11 l'énumère. Il sert à
 * peupler le filtre : une saisie libre laisserait une faute de frappe rendre
 * une liste vide sans dire pourquoi.
 */
/** Commandes que le hub accepte de faire exécuter à une sonde (§ 14). */
export type CommandKind =
  | 'speedtest'
  | 'port_scan'
  | 'discovery'
  | 'add_monitor'
  | 'remove_monitor';

export interface ProbeCommand {
  id: number;
  kind: CommandKind | string;
  args: Record<string, unknown>;
  /** `pending` | `done` | `failed`. */
  state: string;
  created_at: number;
  created_by: string | null;
  delivered_count: number;
  settled_at: number | null;
  error: string | null;
}

export interface MonitorState {
  ip: string;
  alive: boolean;
  /** Dernier relevé. */
  latency_ms?: number | null;
  /**
   * ⚠️ Min / moyenne / max sur ce que la sonde garde en mémoire. La dernière
   * valeur seule ne dit rien de la stabilité : un lien qui oscille entre 5 et
   * 300 ms et un lien stable à 150 affichent la même chose une fois sur deux.
   */
  min_ms?: number | null;
  avg_ms?: number | null;
  max_ms?: number | null;
  uptime_pct: number;
  samples: number;
}

/**
 * Une surveillance telle que le hub la tient, hors mesures.
 *
 * ⚠️ `removed_at` non nul EST la suppression : un fait explicite et daté, qu'on
 * conserve. La ligne n'est jamais retirée — c'est elle qui alimente le
 * sous-onglet « Retirées » et qui rend la réactivation possible. « Retirée » et
 * « jamais connue » doivent rester discernables.
 *
 * Toutes ces dates sont dans la référence du hub : la sonde n'envoie jamais de
 * date mais une ancienneté, que le hub soustrait de sa propre heure.
 */
export interface MonitorEntry {
  target: string;
  /** Epoch en secondes. */
  added_at: number;
  /** Epoch en secondes. `null` = active. */
  removed_at: number | null;
  /** Dernier changement arbitré, epoch en secondes. Le plus récent gagne. */
  last_change_at: number;
}

export interface ScanHost {
  ip: string;
  hostname?: string | null;
  mac?: string | null;
  vendor?: string | null;
  latency_ms?: number | null;
}

export interface ScanPort {
  ip: string;
  port: number;
  proto: string;
  service?: string | null;
}

export interface Scan {
  scan_id: string;
  kind: string;
  started_at: number;
  cidr: string | null;
  hosts: ScanHost[];
  ports: ScanPort[];
}

export interface SpeedtestRow {
  started_at: number;
  engine: string | null;
  server_name: string | null;
  download_mbps: number | null;
  upload_mbps: number | null;
  latency_ms: number | null;
  jitter_ms: number | null;
  result_url: string | null;
}

export const AUDIT_ACTIONS: readonly string[] = [
  'auth.setup',
  'auth.login',
  'auth.logout',
  'access.denied',
  'user.create',
  'user.role',
  'user.password',
  'user.disable',
  'user.enable',
  'site.create',
  'site.rename',
  'site.delete',
  'enroll.code',
  'probe.enroll',
  'probe.reenroll',
  'probe.reenroll_code',
  'probe.update',
  'probe.revoke',
  'probe.rotate',
  'probe.revoke_token',
  'settings.update',
  'settings.retention',
  'settings.advertise',
  'influx.read_token.create',
  'influx.read_token.revoke',
  'notify.webhook',
  'notify.smtp',
  'notify.unconfigure',
  'notify.subscription',
  'notify.test',
  'probe.down',
  'probe.up',
  'probe.public_ip_changed',
  'backup.create',
  'backup.restore',
  'backup.retention',
  'user.password_reset_cli',
  'probe.command',
  'user.scope',
  'auth.totp.enable',
  'auth.totp.disable',
  'auth.passkey.add',
  'auth.passkey.remove',
  'site.archive',
  'site.unarchive',
  'probe.archive',
  'probe.unarchive',
];

// ── Notifications (contrat § 13) ────────────────────────────────────────────

export type WebhookTemplate = 'generic' | 'slack' | 'discord';
export type SmtpSecurity = 'none' | 'starttls' | 'tls';

/**
 * ⚠️ **L'URL n'est jamais rendue** : elle porte à elle seule le droit d'écrire
 * dans le salon. `unreadable` dit qu'une valeur scellée existe mais ne s'ouvre
 * plus — ce n'est pas « jamais réglé ».
 */
export interface WebhookStatus {
  configured: boolean;
  template?: WebhookTemplate;
  unreadable?: boolean;
}

/** ⚠️ Le mot de passe n'est jamais rendu : `password_set` dit seulement qu'il existe. */
export interface SmtpStatus {
  configured: boolean;
  unreadable?: boolean;
  host?: string;
  port?: number;
  security?: SmtpSecurity;
  username?: string | null;
  password_set?: boolean;
  from?: string;
  to?: string[];
}

export interface NotifyStatus {
  delay_secs: number;
  smtp: SmtpStatus;
  webhook: WebhookStatus;
}

/** Verdict d'un envoi de test. `attempted: false` = canal non configuré. */
export interface ChannelOutcome {
  channel: string;
  attempted: boolean;
  ok: boolean;
  error: string | null;
}

/**
 * Corps de `PUT /api/notifications/smtp`.
 *
 * ⚠️ `password` **absent** et `password: null` ne veulent pas dire la même
 * chose : absent garde la valeur scellée, `null` la retire. L'interface n'ayant
 * jamais vu le mot de passe, envoyer un champ vide en corrigeant un port
 * l'effacerait.
 */
export interface SmtpPayload {
  host: string;
  port: number;
  security: SmtpSecurity;
  username?: string | null;
  password?: string | null;
  from: string;
  to: string[];
}

export interface SiteSubscription {
  site_id: string;
  name: string;
  enabled: boolean;
}

export interface ProbeSubscription {
  probe_id: string;
  name: string;
  site_id: string;
  site: string;
  /** Ce qui est réglé SUR la sonde. `null` = elle suit son site. */
  exception: boolean | null;
  /** Ce qui s'applique, héritage déjà résolu par le hub. Ne pas le recalculer. */
  enabled: boolean;
}

export interface Subscriptions {
  sites: SiteSubscription[];
  probes: ProbeSubscription[];
}

/** Une clé d'accès enregistrée. Rien de secret : une clé publique ne l'est pas. */
export interface Passkey {
  id: string;
  label: string;
  created_at: number;
  last_used_at: number | null;
}

export interface HubStatus {
  needs_setup: boolean;
  version: string;
  /**
   * Un réglage lu au démarrage a changé depuis. Le hub continue de tourner
   * avec l'ancien : tant que personne ne redémarre le conteneur, ce qui est
   * enregistré et ce qui s'applique diffèrent.
   */
  restart_required?: boolean;
}

/** Erreur applicative : porte le code HTTP pour distinguer 401 / 409 / reste. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
    /**
     * Le corps de la réponse, tel quel.
     *
     * Le message suffit à afficher un refus ; il ne suffit pas à savoir QUOI
     * FAIRE ensuite. Un 401 de connexion qui porte `totp_required` demande un
     * champ de plus, pas « mauvais identifiants » — deviner la nuance depuis
     * le texte du message serait une analyse de chaîne, donc un bug de plus
     * à chaque reformulation.
     */
    readonly body: unknown = null,
  ) {
    super(message);
    this.name = 'ApiError';
  }

  /** Le hub réclame le second facteur : ce n'est pas un mauvais mot de passe. */
  get needsTotp() {
    return (
      this.status === 401 &&
      typeof this.body === 'object' &&
      this.body !== null &&
      (this.body as { totp_required?: boolean }).totp_required === true
    );
  }

  /**
   * Session absente ou expirée → repasser par l'écran de connexion.
   *
   * ⚠️ **401 seulement.** Un `403` n'est pas une session perdue : c'est un
   * refus de rôle, et se reconnecter n'y changera rien. Les confondre
   * éjectait un observateur vers la page de connexion dès qu'il ouvrait un
   * écran réservé — en boucle, sans jamais lui dire pourquoi.
   */
  get isUnauthorized() {
    return this.status === 401;
  }

  /** Session tombée. C'est le SEUL cas où se reconnecter change quelque chose. */
  get isExpired() {
    return this.status === 401;
  }

  /**
   * Rôle insuffisant. Renvoyer l'écran de connexion ici serait une impasse :
   * les mêmes identifiants donneront le même refus. On montre le message du
   * hub, qui nomme la règle.
   */
  get isForbidden() {
    return this.status === 403;
  }

  /** Nom de sonde déjà pris (contrat § enroll / PATCH). */
  get isConflict() {
    return this.status === 409;
  }

  /** Le hub n'a pas répondu du tout : réseau coupé, conteneur arrêté. */
  get isNetwork() {
    return this.status === 0;
  }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  let res: Response;
  try {
    res = await fetch(path, {
      credentials: 'same-origin',
      // ⚠️ Uniquement pour un corps JSON, sérialisé en chaîne. Un `FormData`
      // doit garder le `Content-Type` que le navigateur pose lui-même : lui
      // seul connaît la limite `multipart/…; boundary=…`, et l'écraser par
      // `application/json` rend l'envoi illisible côté hub.
      headers: typeof init.body === 'string' ? { 'Content-Type': 'application/json' } : undefined,
      ...init,
    });
  } catch {
    // fetch ne rejette que sur erreur réseau : le hub est injoignable.
    throw new ApiError(0, 'network');
  }

  if (res.status === 204) return undefined as T;

  const raw = await res.text();
  let body: unknown = null;
  if (raw) {
    try {
      body = JSON.parse(raw);
    } catch {
      body = raw;
    }
  }

  if (!res.ok) {
    // Le contrat impose `{ "error": "<message lisible>" }` ; on tolère quand
    // même un corps vide ou en texte brut plutôt que d'afficher « undefined ».
    const msg =
      body && typeof body === 'object' && 'error' in body
        ? String((body as { error: unknown }).error)
        : typeof body === 'string' && body.trim()
          ? body.trim()
          : `HTTP ${res.status}`;
    throw new ApiError(res.status, msg, body);
  }

  return body as T;
}

/**
 * Fenêtre demandée : `range` pour une fenêtre glissante, `start`/`stop`
 * (epoch, **secondes**) pour une période précise.
 *
 * ⚠️ Un rapport remis à un client porte sur une période convenue : « les 7
 * derniers jours » donnerait un chiffre différent à chaque ouverture du
 * document.
 */
export interface MetricsWindow {
  range?: string;
  start?: number;
  stop?: number;
}

/**
 * Un seul endroit construit la fenêtre. Le rapport et l'historique des
 * adresses doivent couvrir EXACTEMENT la même période : deux constructions
 * séparées finiraient par en couvrir deux légèrement différentes, et le
 * tableau par adresse contredirait la courbe qu'il légende.
 */
function windowQuery(window: MetricsWindow): URLSearchParams {
  const q = new URLSearchParams();
  if (window.start != null && window.stop != null) {
    q.set('start', String(window.start));
    q.set('stop', String(window.stop));
  } else {
    q.set('range', window.range ?? '-24h');
  }
  return q;
}

export const api = {
  /** Route publique, seule accessible quand `needs_setup` est vrai. */
  status: () => request<HubStatus>('/api/status'),

  setup: (setup_token: string, username: string, password: string) =>
    request<unknown>('/api/setup', {
      method: 'POST',
      body: JSON.stringify({ setup_token, username, password }),
    }),

  /**
   * ⚠️ Le code du second facteur part avec le mot de passe, pas dans un
   * second appel qui s'appuierait sur une « connexion à moitié faite » côté
   * hub : un tel état expire, se nettoie, et devient une file de sessions à
   * demi ouvertes qu'on peut remplir de l'extérieur.
   */
  login: (username: string, password: string, code?: string) =>
    request<unknown>('/api/login', {
      method: 'POST',
      body: JSON.stringify(code ? { username, password, code } : { username, password }),
    }),

  /** Second facteur du compte connecté (contrat § 19). */
  totpStatus: () => request<{ enabled: boolean; pending: boolean }>('/api/me/totp'),

  /**
   * Tire un secret et rend de quoi l'enrôler.
   *
   * ⚠️ Le secret ne sort qu'ICI, une fois, et le second facteur n'est PAS
   * encore actif : il le devient à la confirmation.
   */
  totpStart: () =>
    request<{ secret: string; uri: string; qr_svg: string }>('/api/me/totp/start', {
      method: 'POST',
    }),

  totpConfirm: (code: string) =>
    request<{ enabled: boolean }>('/api/me/totp/confirm', {
      method: 'POST',
      body: JSON.stringify({ code }),
    }),

  /** Le mot de passe est exigé : une session volée ne doit pas suffire. */
  totpDisable: (password: string) =>
    request<{ enabled: boolean }>('/api/me/totp', {
      method: 'DELETE',
      body: JSON.stringify({ password }),
    }),

  // ── Clés d'accès (contrat § 19) ─────────────────────────────────────────

  passkeys: () =>
    request<{ passkeys: Passkey[]; secure_context: boolean; rp_id: string }>('/api/me/passkeys'),

  passkeyStart: () => request<unknown>('/api/me/passkeys/start', { method: 'POST' }),

  passkeyFinish: (credential: unknown, label: string) =>
    request<{ ok: boolean }>('/api/me/passkeys/finish', {
      method: 'POST',
      body: JSON.stringify({ credential, label }),
    }),

  passkeyDelete: (id: string) =>
    request<{ ok: boolean }>(`/api/me/passkeys/${encodeURIComponent(id)}`, { method: 'DELETE' }),

  /** Ouvre une cérémonie de connexion par clé. `404` = ce compte n'en a pas. */
  passkeyLoginStart: (username: string) =>
    request<unknown>('/api/login/passkey/start', {
      method: 'POST',
      body: JSON.stringify({ username }),
    }),

  passkeyLoginFinish: (username: string, credential: unknown) =>
    request<unknown>('/api/login/passkey/finish', {
      method: 'POST',
      body: JSON.stringify({ username, credential }),
    }),

  /** Qui est connecté, et avec quel rôle. `401` si la session ne tient plus. */
  me: () => request<Identity>('/api/me'),

  logout: () => request<unknown>('/api/logout', { method: 'POST' }),

  /** `archived` inclut les sites archivés — exclus par défaut. */
  sites: (archived = false) =>
    request<Site[]>(archived ? '/api/sites?archived=true' : '/api/sites'),

  createSite: (name: string) =>
    request<Site>('/api/sites', { method: 'POST', body: JSON.stringify({ name }) }),

  renameSite: (id: string, name: string) =>
    request<Site | null>(`/api/sites/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: JSON.stringify({ name }),
    }),

  /** `siteId` absent = tout le parc ; sinon le hub filtre côté serveur. */
  probes: (siteId?: string, archived = false) => {
    const q = new URLSearchParams();
    if (siteId) q.set('site', siteId);
    if (archived) q.set('archived', 'true');
    const qs = q.toString();
    return request<Probe[]>(qs ? `/api/probes?${qs}` : '/api/probes');
  },

  /**
   * Retire un site du parc — ou l'y remet — **sans rien supprimer**.
   *
   * 🔴 C'est la réponse à « comment je fais partir un client ». Le supprimer
   * vraiment rendrait ses mesures orphelines : son rapport SLA de l'année
   * écoulée n'aurait plus de site auquel se rattacher. Ici tout reste, et
   * l'opération se défait. Les sondes du site suivent.
   */
  archiveSite: (id: string, archived: boolean) =>
    request<{ ok: boolean; probes: number }>(
      `/api/sites/${encodeURIComponent(id)}/archive`,
      { method: 'POST', body: JSON.stringify({ archived }) },
    ),

  archiveProbe: (id: string, archived: boolean) =>
    request<{ ok: boolean }>(`/api/probes/${encodeURIComponent(id)}/archive`, {
      method: 'POST',
      body: JSON.stringify({ archived }),
    }),

  /** Les deux champs sont facultatifs : renommage, déplacement, ou les deux. */
  patchProbe: (id: string, patch: { name?: string; site_id?: string }) =>
    request<Probe | null>(`/api/probes/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: JSON.stringify(patch),
    }),

  /** Révoque le jeton de la sonde. Ne supprime AUCUNE mesure (contrat § 3). */
  revoke: (id: string) =>
    request<unknown>(`/api/probes/${encodeURIComponent(id)}`, { method: 'DELETE' }),

  /** Code d'enrôlement court, lié à un site, valable 15 minutes (contrat § 3). */
  createEnrollCode: (siteId: string) =>
    request<EnrollCode>('/api/enroll-codes', {
      method: 'POST',
      body: JSON.stringify({ site_id: siteId }),
    }),

  /**
   * Enrôlements encore ouverts. Un hub antérieur à cette route répond 404 :
   * l'appelant doit traiter l'absence comme « rien à afficher côté serveur »,
   * pas comme une panne du parc.
   */
  pendingEnrollCodes: () => request<PendingEnroll[]>('/api/enroll-codes/pending'),

  /**
   * Code de ré-enrôlement rattaché à une sonde EXISTANTE : elle revient avec le
   * même `probe_id`, donc tout son historique (contrat § 9).
   */
  reenrollCode: (id: string) =>
    request<EnrollCode>(`/api/probes/${encodeURIComponent(id)}/reenroll-code`, {
      method: 'POST',
    }),

  /**
   * Arme ou désarme le mode temps réel sur une sonde.
   *
   * ⚠️ Le hub ne joint jamais la sonde : la nouvelle cadence voyage dans la
   * réponse au battement. La réponse dit donc « c'est armé », pas « la sonde a
   * changé de rythme » — il s'écoule jusqu'à une minute entre les deux, et
   * l'écran doit l'annoncer.
   */
  setRealtime: (id: string, on: boolean) =>
    request<{ ok: boolean; realtime_until: number | null }>(
      `/api/probes/${encodeURIComponent(id)}/realtime`,
      { method: 'POST', body: JSON.stringify({ on }) },
    ),

  /**
   * Retire une surveillance **depuis le hub**. Le retrait est un fait daté,
   * écrit IMMÉDIATEMENT — pas une commande qui dort dans la file.
   *
   * ⚠️ C'est toute la différence, et c'est le scénario métier : retirer une
   * cible pendant que la sonde est **éteinte**. Une commande n'agit qu'au
   * retour de la sonde, et se perd si elle ne revient pas. La route écrit la
   * suppression tout de suite, et l'arbitrage du contrat §20 — le plus récent
   * gagne — la fait tenir face à la liste que la sonde rapportera : celle
   * d'une machine éteinte est par construction antérieure.
   *
   * ⚠️ Même remarque que `reviveMonitor` : le hub ne joint jamais la sonde. La
   * réponse dit « la suppression est écrite côté hub », pas « la sonde a cessé
   * de pinguer » — elle recevra la liste effective à son prochain battement.
   *
   * `applied` à faux = un changement plus récent avait déjà tranché ; rien n'a
   * été écrit, et l'écran doit le dire plutôt qu'annoncer un retrait.
   */
  removeMonitor: (id: string, target: string) =>
    request<{ ok: boolean; applied: boolean }>(
      `/api/probes/${encodeURIComponent(id)}/monitors/remove`,
      { method: 'POST', body: JSON.stringify({ target }) },
    ),

  /**
   * Réactive une surveillance retirée. **Réactiver, c'est effacer la
   * suppression** — il n'y a pas d'autre mécanisme.
   *
   * ⚠️ Même remarque que pour le mode temps réel : le hub ne joint jamais la
   * sonde. La réponse dit « la suppression est effacée côté hub », pas « la
   * sonde pingue de nouveau » — elle reprendra la cible à son prochain
   * battement, et l'écran doit le dire.
   *
   * `applied` à faux = un changement plus récent avait déjà tranché ; rien n'a
   * été écrit.
   */
  reviveMonitor: (id: string, target: string) =>
    request<{ ok: boolean; applied: boolean }>(
      `/api/probes/${encodeURIComponent(id)}/monitors/revive`,
      { method: 'POST', body: JSON.stringify({ target }) },
    ),

  /** Rotation d'hygiène : l'ancienne clé ne meurt qu'après accusé de la sonde. */
  rotateToken: (id: string) =>
    request<unknown>(`/api/probes/${encodeURIComponent(id)}/rotate`, { method: 'POST' }),

  /** Compromission : l'ancienne clé meurt immédiatement. */
  revokeToken: (id: string) =>
    request<unknown>(`/api/probes/${encodeURIComponent(id)}/revoke-token`, { method: 'POST' }),

  /**
   * État d'Influx en lecture seule.
   *
   * ⚠️ Le contrat § 8 décrit les champs à afficher mais ne nomme pas la route
   * qui les sert. `GET /api/influx` est le nom retenu côté interface, à figer
   * avec l'implémentation backend. Les champs manquants sont rendus « — »
   * plutôt que de faire échouer l'écran.
   */
  influx: async (): Promise<InfluxInfo> => {
    const raw = (await request<Record<string, unknown>>('/api/influx')) ?? {};
    const num = (v: unknown) => (typeof v === 'number' && Number.isFinite(v) ? v : null);
    return {
      url: String(raw.url ?? ''),
      org: String(raw.org ?? ''),
      bucket: String(raw.bucket ?? ''),
      version: raw.version != null ? String(raw.version) : null,
      healthy:
        typeof raw.healthy === 'boolean'
          ? raw.healthy
          : typeof raw.health === 'string'
            ? raw.health === 'pass' || raw.health === 'ok'
            : null,
      // Le hub nomme la taille `bucket_bytes` ; les deux autres noms restent
      // acceptés plutôt que d'afficher « — » sur une valeur qui existe.
      size_bytes: num(raw.size_bytes) ?? num(raw.bucket_bytes) ?? num(raw.bytes),
      series_count: num(raw.series_count) ?? num(raw.series),
    };
  },

  readTokens: () => request<ReadToken[]>('/api/influx/read-tokens'),

  /** La valeur du jeton n'est renvoyée qu'ici, une seule fois. */
  createReadToken: (description: string) =>
    request<{ auth_id: string; token: string; description?: string }>('/api/influx/read-token', {
      method: 'POST',
      body: JSON.stringify({ description }),
    }),

  revokeReadToken: (tokenId: string) =>
    request<unknown>(`/api/influx/read-tokens/${encodeURIComponent(tokenId)}`, {
      method: 'DELETE',
    }),

  /** Réglages du hub. Les valeurs absentes retombent sur les défauts du contrat. */
  settings: async (): Promise<HubSettings> => {
    const raw = await request<Partial<HubSettings>>('/api/settings');
    return { ...SETTINGS_DEFAULTS, ...(raw ?? {}) };
  },

  /**
   * Mise à jour partielle : on n'envoie que ce que l'utilisateur a changé.
   *
   * `confirm_data_loss` est exigé par le hub pour toute réduction de la
   * rétention — elle supprime des mesures. Sans ce drapeau, il répond 409.
   */
  saveSettings: (
    patch: Partial<HubSettings> & { confirm_data_loss?: boolean },
  ) => request<Partial<HubSettings>>('/api/settings', {
    method: 'PUT',
    body: JSON.stringify(patch),
  }),

  /**
   * `reachable: false` revient en HTTP 200 : c'est un verdict de test, pas une
   * panne de l'interface. Ne jamais le router vers la gestion d'erreur réseau.
   */
  testAdvertise: () =>
    request<AdvertiseTest>('/api/settings/influx-advertise/test', { method: 'POST' }),

  // ── Inventaire et commandes (contrat § 12 et § 14) ───────────────────────

  inventory: <T>(id: string, kind: 'ports' | 'discovery' | 'speedtest') =>
    request<T>(
      `/api/probes/${encodeURIComponent(id)}/inventory?kind=${kind}`,
    ),

  /** Relevés bruts d'une fenêtre, pour le rapport SLA. */
  sla: (id: string, window: MetricsWindow) =>
    request<import('./sla-report').SlaPayload>(
      `/api/probes/${encodeURIComponent(id)}/sla?${windowQuery(window)}`,
    ),

  /**
   * Intervalles d'adresse publique de la fenêtre — et rien d'autre.
   *
   * ⚠️ Route séparée, et non le rapport complet : `/sla` embarque le scan de
   * découverte, les ports ouverts et l'historique des débits. Le rappeler
   * toutes les 30 secondes pour deux colonnes ferait transiter un gros objet
   * en boucle. Même fenêtre que `/sla`, donc mêmes bornes exactement.
   */
  publicIps: (id: string, window: MetricsWindow) =>
    request<{ intervals: import('./sla-report').PublicIpInterval[] }>(
      `/api/probes/${encodeURIComponent(id)}/public-ips?${windowQuery(window)}`,
    ),

  /**
   * Nomme un réseau. Le libellé vit dans sa propre table côté hub : il survit
   * à la purge des intervalles et se réapplique tout seul le jour où la sonde
   * revient sur ce réseau.
   */
  setNetworkLabel: (body: NetworkLabel) =>
    request<{ ok: boolean }>('/api/networks/label', {
      method: 'PUT',
      body: JSON.stringify(body),
    }),

  commands: (id: string) =>
    request<{ commands: ProbeCommand[] }>(
      `/api/probes/${encodeURIComponent(id)}/commands`,
    ),

  /**
   * Empile une commande. ⚠️ Elle n'est PAS exécutée tout de suite : le hub
   * n'a aucune route vers la sonde, la commande part dans la réponse au
   * prochain battement de cœur. L'interface doit le dire, sinon on croit la
   * commande perdue pendant la minute qui suit.
   */
  runCommand: (id: string, kind: CommandKind, args: Record<string, unknown> = {}) =>
    request<{ id: number; kind: string; state: string; heartbeat_interval_secs: number }>(
      `/api/probes/${encodeURIComponent(id)}/commands`,
      { method: 'POST', body: JSON.stringify({ kind, args }) },
    ),

  /**
   * Mesures d'une fenêtre — même construction de bornes que `/sla`.
   *
   * `maxPoints` est la **largeur du graphe en pixels**. C'est elle, et elle
   * seule, qui décide si le hub agrège et à quel pas : ⚠️ ni l'ancienneté de la
   * plage ni sa durée n'entrent en compte. Dix minutes prises il y a trois mois
   * reviennent brutes ; quatre-vingts jours reviennent moyennés, avec le
   * maximum de chaque intervalle à côté et `aggregated_every` dans la réponse.
   */
  metrics: (id: string, measurement: string, window: MetricsWindow, maxPoints: number) => {
    const q = windowQuery(window);
    q.set('measurement', measurement);
    q.set('max_points', String(Math.max(1, Math.round(maxPoints))));
    return request<unknown>(`/api/probes/${encodeURIComponent(id)}/metrics?${q}`);
  },

  // ── Comptes ──────────────────────────────────────────────────────────────
  //
  // Il n'y a pas de `deleteUser`, et ce n'est pas un oubli : le hub répond 405
  // à `DELETE /api/users/{…}`, définitivement.

  users: () => request<Account[]>('/api/users'),

  createUser: (username: string, password: string, role: Role) =>
    request<Account>('/api/users', {
      method: 'POST',
      body: JSON.stringify({ username, password, role }),
    }),

  /**
   * Rôle, activation, portée, ou tout à la fois. Le hub refuse (409) ce qui
   * couperait le dernier accès, et refuse de restreindre un `admin`.
   *
   * ⚠️ `sites: []` est un ORDRE — « plus aucune restriction » — alors que
   * l'absence du champ ne touche pas à la portée existante.
   */
  patchUser: (
    username: string,
    patch: { role?: Role; disabled?: boolean; sites?: string[] },
  ) =>
    request<Account>(`/api/users/${encodeURIComponent(username)}`, {
      method: 'PATCH',
      body: JSON.stringify(patch),
    }),

  /**
   * Change SON PROPRE mot de passe.
   *
   * ⚠️ L'ancien est exigé par le hub : une session volée suffirait sinon à
   * s'approprier le compte définitivement.
   */
  changeOwnPassword: (current: string, next: string) =>
    request<{ ok: boolean }>('/api/me/password', {
      method: 'POST',
      body: JSON.stringify({ current, next }),
    }),

  setUserPassword: (username: string, password: string) =>
    request<{ ok: boolean }>(`/api/users/${encodeURIComponent(username)}/password`, {
      method: 'POST',
      body: JSON.stringify({ password }),
    }),

  // ── Journal d'audit ──────────────────────────────────────────────────────
  //
  // Lecture seule. Pas de `deleteAudit` : `DELETE /api/audit` rend 405.

  /**
   * Pagination à rebours. `before_id` reprend là où la page précédente s'est
   * arrêtée : un `OFFSET` rejouerait des lignes déjà vues dès qu'une ligne
   * s'ajoute pendant la lecture.
   */
  audit: (q: {
    limit?: number;
    before_id?: number | null;
    actor?: string;
    action?: string;
  } = {}) => {
    const p = new URLSearchParams();
    if (q.limit != null) p.set('limit', String(q.limit));
    if (q.before_id != null) p.set('before_id', String(q.before_id));
    if (q.actor?.trim()) p.set('actor', q.actor.trim());
    if (q.action?.trim()) p.set('action', q.action.trim());
    const qs = p.toString();
    return request<AuditPage>(`/api/audit${qs ? `?${qs}` : ''}`);
  },

  // ── Notifications ────────────────────────────────────────────────────────

  notifications: () => request<NotifyStatus>('/api/notifications'),

  /**
   * ⚠️ Ne pas passer `url` quand l'utilisateur n'a rien saisi : le champ absent
   * garde l'URL scellée, un champ vide la remplacerait par du vide.
   */
  saveWebhook: (body: { template: WebhookTemplate; url?: string }) =>
    request<NotifyStatus>('/api/notifications/webhook', {
      method: 'PUT',
      body: JSON.stringify(body),
    }),

  clearWebhook: () =>
    request<NotifyStatus>('/api/notifications/webhook', { method: 'DELETE' }),

  saveSmtp: (body: SmtpPayload) =>
    request<NotifyStatus>('/api/notifications/smtp', {
      method: 'PUT',
      body: JSON.stringify(body),
    }),

  clearSmtp: () => request<NotifyStatus>('/api/notifications/smtp', { method: 'DELETE' }),

  /** Envoie un VRAI message sur chaque canal configuré, et rend le verdict de chacun. */
  testNotifications: () =>
    request<{ channels: ChannelOutcome[] }>('/api/notifications/test', { method: 'POST' }),

  /** Héritage déjà résolu par le hub — `enabled` est la valeur qui s'applique. */
  subscriptions: () => request<Subscriptions>('/api/notifications/subscriptions'),

  // ── Sauvegarde ───────────────────────────────────────────────────────────

  backups: async (): Promise<BackupList> => {
    const raw = (await request<Partial<BackupList>>('/api/backups')) ?? {};
    return {
      dir: raw.dir ?? '',
      enabled: raw.enabled ?? true,
      interval_hours: raw.interval_hours ?? 24,
      keep_last: raw.keep_last ?? 7,
      backups: raw.backups ?? [],
    };
  },

  /** Sauvegarde immédiate, puis application de la rétention par le hub. */
  createBackup: () => request<BackupCreated>('/api/backup', { method: 'POST' }),

  /**
   * Restaure une archive **déjà dans le volume**. `confirm_overwrite` est
   * exigé dès qu'il y a des données en place : sans lui le hub répond 409 et
   * ne touche à rien.
   */
  restoreBackup: (file: string, confirm_overwrite: boolean) =>
    request<RestoreReport>(`/api/backup/restore/${encodeURIComponent(file)}`, {
      method: 'POST',
      body: JSON.stringify({ confirm_overwrite }),
    }),

  /**
   * Restaure une archive téléversée — le chemin principal.
   *
   * ⚠️ `FormData` sans `Content-Type` explicite : c'est le navigateur qui doit
   * poser la limite `multipart/boundary=…`. La fixer à la main produit un
   * corps que le hub lit comme « envoi illisible ».
   */
  restoreUpload: (archive: File, confirm_overwrite: boolean) => {
    const form = new FormData();
    form.append('confirm_overwrite', String(confirm_overwrite));
    form.append('archive', archive, archive.name);
    return request<RestoreReport>('/api/backup/restore', { method: 'POST', body: form });
  },

  /** `enabled: null` sur une sonde = elle revient à l'héritage de son site. */
  setSubscription: (scope: 'site' | 'probe', scope_id: string, enabled: boolean | null) =>
    request<{ ok: boolean }>('/api/notifications/subscriptions', {
      method: 'PUT',
      body: JSON.stringify({ scope, scope_id, enabled }),
    }),
};

/**
 * Qui est connecté, ou `null` si la session ne tient plus.
 *
 * `/api/status` est publique et ne dit rien de la session ; `/api/me` est la
 * route qui répond aux deux questions d'un coup — « la session tient-elle ? »
 * et « avec quel rôle ? ». Elle remplace l'ancien sondage de `/api/probes`, qui
 * ne rendait que la première et faisait payer une liste de sondes pour l'avoir.
 *
 * Une erreur autre que `401` **remonte** : hub injoignable et session expirée
 * n'appellent pas la même réponse, et confondre les deux enverrait à l'écran de
 * connexion quelqu'un dont le réseau est simplement coupé.
 */
export async function whoami(): Promise<Identity | null> {
  try {
    return await api.me();
  } catch (e) {
    if (e instanceof ApiError && e.isExpired) return null;
    throw e;
  }
}
