/**
 * La bande de battement 24 h — la signature de la DA « Relevé » — et la perte
 * en niveaux de gris de la fiche.
 *
 * 96 quarts d'heure par sonde, au même endroit sur chaque ligne du parc : une
 * colonne orange qui traverse les sondes d'un site, c'est le lien du site, pas
 * une sonde. La santé se tait (vert sourd) ; seul le défaut prend la couleur.
 *
 * ⚠️ **Aucune route nouvelle.** La bande se lit sur
 * `GET /api/probes/{id}/metrics?measurement=internet_status&range=-24h` : le
 * champ texte `state` passe brut (le hub ne moyenne pas du texte), on garde
 * donc le pire état de chaque quart d'heure. Le battement lui-même n'est pas
 * historisé côté hub — seul `last_seen` l'est ; c'est lui qui date le silence
 * en cours.
 */

import type { MonitorState } from './api';
import { probeLiveness } from './probe-liveness';

/** `ok` bat normalement · `w` limité · `b` coupé ou hors ligne · `n` sans relevé · `a` temps réel. */
export type Beat = 'ok' | 'w' | 'b' | 'n' | 'a';

export const STRIP_CELLS = 96;
export const STRIP_SPAN_MS = 24 * 3_600_000;

export interface HeartbeatInput {
  /** Relevés `state` d'`internet_status`, en millisecondes. */
  points: { t: number; v: number | string }[];
  from: number;
  to: number;
  cells?: number;
  /** Enrôlement, en secondes epoch : ce qui précède n'a pas à être jugé. */
  createdAt: number | null | undefined;
  /** Dernier battement, en secondes epoch. */
  lastSeen: number | null | undefined;
  /** Horloge, pour décider si le silence en cours est déjà « hors ligne ». */
  atMs: number;
  realtimeUntil?: number | null;
}

const RANK: Record<string, number> = { online: 1, limited: 2, offline: 3 };

export function heartbeatCells(input: HeartbeatInput): Beat[] {
  const n = input.cells ?? STRIP_CELLS;
  const step = (input.to - input.from) / n;
  const worst = new Array<number>(n).fill(0);
  for (const p of input.points) {
    if (p.t < input.from || p.t >= input.to) continue;
    const i = Math.min(n - 1, Math.floor((p.t - input.from) / step));
    const r = RANK[String(p.v).toLowerCase()] ?? 0;
    if (r > worst[i]) worst[i] = r;
  }

  const created = input.createdAt != null ? input.createdAt * 1000 : null;
  const last = input.lastSeen != null ? input.lastSeen * 1000 : null;
  // 🔴 Le silence en cours n'est rouge que si la sonde est HORS LIGNE : une
  // sonde seulement silencieuse ne prouve encore rien, on ne l'affirme pas.
  const offlineNow =
    input.lastSeen != null && probeLiveness(input.lastSeen, input.atMs).state === 'offline';

  const out: Beat[] = [];
  for (let i = 0; i < n; i++) {
    const start = input.from + i * step;
    const end = start + step;
    if (created != null && end <= created) {
      out.push('n');
    } else if (worst[i] === 3) {
      out.push('b');
    } else if (worst[i] === 2) {
      out.push('w');
    } else if (worst[i] === 1) {
      out.push('ok');
    } else if (offlineNow && last != null && end > last) {
      out.push('b');
    } else {
      out.push('n');
    }
  }
  if (input.realtimeUntil != null && input.realtimeUntil * 1000 > input.atMs && out[n - 1] !== 'b') {
    out[n - 1] = 'a';
  }
  return out;
}

/** Niveau de gris d'une perte en pourcentage : 0, < 1 %, 1 à 5 %, 5 à 20 %, > 20 %. */
export function lossLevel(pct: number): 0 | 1 | 2 | 3 | 4 {
  if (pct <= 0) return 0;
  if (pct < 1) return 1;
  if (pct < 5) return 2;
  if (pct <= 20) return 3;
  return 4;
}

/**
 * Perte par case, en pourcentage, depuis `alive` (1 = répond, 0 = perdu ; ou
 * la moyenne de l'intervalle quand le hub agrège). `null` = aucun relevé :
 * pas de case, plutôt qu'un 0 % qui affirmerait.
 */
export function lossCells(
  points: { t: number; v: number }[],
  from: number,
  to: number,
  cells: number,
): (number | null)[] {
  const step = (to - from) / cells;
  const sum = new Array<number>(cells).fill(0);
  const count = new Array<number>(cells).fill(0);
  for (const p of points) {
    if (p.t < from || p.t >= to || !Number.isFinite(p.v)) continue;
    const i = Math.min(cells - 1, Math.floor((p.t - from) / step));
    sum[i] += p.v;
    count[i]++;
  }
  return sum.map((s, i) => (count[i] ? (1 - s / count[i]) * 100 : null));
}

type Gateway = { gateway?: string | null; monitors?: MonitorState[] | null };

/** La cible qui représente le lien : la passerelle si elle est surveillée, sinon la première. */
function pick<T>(gateway: string | null | undefined, entries: [string, T][]): T | null {
  if (entries.length === 0) return null;
  return (entries.find(([ip]) => ip === gateway) ?? entries[0])[1];
}

/** Latence passerelle de la ligne du parc, en ms — la moyenne de la sonde quand elle la donne. */
export function gatewayLatency(p: Gateway): number | null {
  const m = pick(
    p.gateway,
    (p.monitors ?? []).map((x) => [x.ip, x] as [string, MonitorState]),
  );
  if (!m) return null;
  return m.avg_ms ?? m.latency_ms ?? null;
}

/** Perte 24 h de la passerelle, en %, depuis `/uptime`. `null` quand on ne sait pas. */
export function gatewayLoss(
  p: Gateway,
  uptime: Record<string, { uptime_pct: number | null; samples: number }>,
): number | null {
  const row = pick(p.gateway, Object.entries(uptime));
  if (!row || row.uptime_pct == null) return null;
  return Math.max(0, 100 - row.uptime_pct);
}
