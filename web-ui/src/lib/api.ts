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
}

export const SETTINGS_DEFAULTS: HubSettings = {
  influx_url: 'https://127.0.0.1:8086',
  influx_org: 'lanprobe',
  influx_bucket: 'lanprobe',
  influx_advertise_url: null,
  hub_public_url: null,
  retention_days: 0,
  heartbeat_interval_secs: 60,
};

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

  /** Session absente ou expirée → l'UI doit repasser par l'écran de connexion. */
  get isUnauthorized() {
    return this.status === 401 || this.status === 403;
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
      headers: init.body ? { 'Content-Type': 'application/json' } : undefined,
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

  metrics: (id: string, measurement: string, range: string) =>
    request<unknown>(
      `/api/probes/${encodeURIComponent(id)}/metrics` +
        `?measurement=${encodeURIComponent(measurement)}&range=${encodeURIComponent(range)}`,
    ),
};

/**
 * Le contrat ne prévoit pas de champ « authentifié » dans `/api/status` : la
 * seule façon conforme de savoir si la session tient est de demander une route
 * protégée et de regarder si elle répond 401.
 */
export async function probeSession(): Promise<boolean> {
  try {
    await api.probes();
    return true;
  } catch (e) {
    if (e instanceof ApiError && e.isUnauthorized) return false;
    throw e;
  }
}
