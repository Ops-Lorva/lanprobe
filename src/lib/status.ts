/**
 * Les états affichés par l'app : une forme, un ton, un mot.
 *
 * Mêmes formes que le hub (`web-ui/src/lib/components/StatusMark.svelte`) :
 *   — disque plein : présent, rien à faire ;
 *   — demi-disque : ça marche, mais à moitié ;
 *   — anneau : on ne sait pas (pas encore, ou plus) ;
 *   — carré : absent. Le seul état qui appelle une action.
 * La forme seule suffit à balayer une colonne, en niveaux de gris compris.
 */
export type Tone = 'ok' | 'warn' | 'bad' | 'none';
export type Shape = 'dot' | 'half' | 'ring' | 'square';

export interface Mark {
  tone: Tone;
  shape: Shape;
  /** Clé i18n du mot. */
  key: string;
  values?: Record<string, string | number>;
}

/** Une cible surveillée par ping. `null` : pas encore de mesure. */
export function targetMark(alive: boolean | null | undefined): Mark {
  if (alive == null) return { tone: 'none', shape: 'ring', key: 'status.target_pending' };
  return alive
    ? { tone: 'ok', shape: 'dot', key: 'status.target_alive' }
    : { tone: 'bad', shape: 'square', key: 'status.target_down' };
}

export type InternetState = 'online' | 'limited' | 'offline';

export function internetMark(state: InternetState | null | undefined): Mark {
  switch (state) {
    case 'online':
      return { tone: 'ok', shape: 'dot', key: 'status.internet_online' };
    case 'limited':
      return { tone: 'warn', shape: 'half', key: 'status.internet_limited' };
    case 'offline':
      return { tone: 'bad', shape: 'square', key: 'status.internet_offline' };
    default:
      return { tone: 'none', shape: 'ring', key: 'status.unknown' };
  }
}

/** Ce que renvoie `cmd_hub_status`. */
export interface HubStatus {
  enrolled?: boolean;
  url: string;
  name?: string;
  site?: string;
  state: string;
  last_beat_at?: number | null;
  heartbeat_interval_secs?: number | null;
  last_error: string | null;
  buffered_points: number;
  refused_status: number | null;
}

/**
 * Le rattachement au hub.
 *
 * Trois conduites différentes, à ne pas confondre : `degraded` se répare seul
 * au retour du lien, `broken` jamais — le hub a refusé le jeton, il faut
 * ré-enrôler. Et `degraded` se dit de deux façons, parce qu'on ne cherche pas
 * au même endroit : rien n'a répondu, ou quelque chose a répondu et a refusé
 * (un 403 de proxy devant un hub sain, le 02/09).
 */
export function hubMark(h: HubStatus | null | undefined): Mark {
  if (!h || (!h.enrolled && h.state === 'off')) {
    return { tone: 'none', shape: 'ring', key: 'hub_state.detached' };
  }
  switch (h.state) {
    case 'ok':
      return { tone: 'ok', shape: 'dot', key: 'hub_state.attached' };
    case 'broken':
      return { tone: 'bad', shape: 'square', key: 'hub_state.revoked' };
    case 'degraded':
      return h.refused_status != null
        ? { tone: 'warn', shape: 'ring', key: 'hub_state.refused', values: { code: h.refused_status, n: h.buffered_points } }
        : { tone: 'warn', shape: 'ring', key: 'hub_state.unreachable', values: { n: h.buffered_points } };
    default:
      return { tone: 'none', shape: 'ring', key: 'hub_state.pending' };
  }
}
