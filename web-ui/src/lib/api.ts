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
}

export interface Probe {
  probe_id: string;
  name: string;
  site_id: string;
  site: string;
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
   * Rotation de clé en attente de remise (contrat § 9). Champs facultatifs :
   * ils ne figurent pas encore dans l'exemple de `GET /api/probes`, l'interface
   * ne les affiche donc que si le hub les fournit.
   */
  token_rotation_pending?: boolean;
  token_version?: number;
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
  /** Sauvegarde planifiée. Coupée, plus aucune archive n'est produite seule. */
  backup_enabled: boolean;
  backup_interval_hours: number;
  /** Archives conservées. `0` = toutes ; baisser la valeur en supprime. */
  backup_keep_last: number;
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
  backup_enabled: true,
  backup_interval_hours: 24,
  backup_keep_last: 7,
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
  latency_ms?: number | null;
  uptime_pct: number;
  samples: number;
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

export interface HubStatus {
  needs_setup: boolean;
  version: string;
}

/** Erreur applicative : porte le code HTTP pour distinguer 401 / 409 / reste. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = 'ApiError';
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
    throw new ApiError(res.status, msg);
  }

  return body as T;
}

export const api = {
  /** Route publique, seule accessible quand `needs_setup` est vrai. */
  status: () => request<HubStatus>('/api/status'),

  setup: (setup_token: string, username: string, password: string) =>
    request<unknown>('/api/setup', {
      method: 'POST',
      body: JSON.stringify({ setup_token, username, password }),
    }),

  login: (username: string, password: string) =>
    request<unknown>('/api/login', {
      method: 'POST',
      body: JSON.stringify({ username, password }),
    }),

  /** Qui est connecté, et avec quel rôle. `401` si la session ne tient plus. */
  me: () => request<Identity>('/api/me'),

  logout: () => request<unknown>('/api/logout', { method: 'POST' }),

  sites: () => request<Site[]>('/api/sites'),

  createSite: (name: string) =>
    request<Site>('/api/sites', { method: 'POST', body: JSON.stringify({ name }) }),

  renameSite: (id: string, name: string) =>
    request<Site | null>(`/api/sites/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: JSON.stringify({ name }),
    }),

  /** `siteId` absent = tout le parc ; sinon le hub filtre côté serveur. */
  probes: (siteId?: string) =>
    request<Probe[]>(siteId ? `/api/probes?site=${encodeURIComponent(siteId)}` : '/api/probes'),

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
  /**
   * `range` pour une fenêtre glissante, `start`/`stop` (epoch, secondes) pour
   * une période précise. ⚠️ Un rapport remis à un client porte sur une période
   * convenue : « les 7 derniers jours » donnerait un chiffre différent à
   * chaque ouverture du document.
   */
  sla: (id: string, window: { range?: string; start?: number; stop?: number }) => {
    const q = new URLSearchParams();
    if (window.start != null && window.stop != null) {
      q.set('start', String(window.start));
      q.set('stop', String(window.stop));
    } else {
      q.set('range', window.range ?? '-24h');
    }
    return request<import('./sla-report').SlaPayload>(
      `/api/probes/${encodeURIComponent(id)}/sla?${q}`,
    );
  },

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

  metrics: (id: string, measurement: string, range: string) =>
    request<unknown>(
      `/api/probes/${encodeURIComponent(id)}/metrics` +
        `?measurement=${encodeURIComponent(measurement)}&range=${encodeURIComponent(range)}`,
    ),

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

  /** Rôle, activation, ou les deux. Le hub refuse (409) ce qui couperait le dernier accès. */
  patchUser: (username: string, patch: { role?: Role; disabled?: boolean }) =>
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
