import { writable, get } from 'svelte/store';
import { api, ApiError } from './api';
import { normalizeCurves, type Point } from './metrics';
import { normalizeUptime, type TargetUptime } from './uptime';
import { STRIP_CELLS } from './heartbeat';

/**
 * Ce que la ligne du parc lit en plus de `/api/probes` : 24 h d'état internet
 * (la bande de battement) et le taux par cible (la perte 24 h).
 *
 * ⚠️ **À part du parc, et lentement.** La bande compte des quarts d'heure : la
 * relire toutes les 30 s avec le parc ferait un appel Influx par sonde pour
 * rien. Une sonde se relit au plus toutes les 5 min, quatre à la fois — un
 * parc de cinquante sondes ne part pas d'un coup sur le hub.
 *
 * Un échec ne dit rien à l'écran : la bande reste hachurée (« sans relevé »),
 * ce qui est exactement ce qu'on sait.
 */

export interface ProbeBeats {
  states: Point[];
  uptime: Record<string, TargetUptime>;
  loadedAt: number;
}

const MAX_AGE_MS = 5 * 60_000;
const PARALLEL = 4;

function create() {
  const store = writable<Record<string, ProbeBeats>>({});
  const inFlight = new Set<string>();

  async function readOne(id: string, onExpired: () => void) {
    const window = { range: '-24h' };
    try {
      const [raw, up] = await Promise.all([
        api.metrics(id, 'internet_status', window, STRIP_CELLS),
        api.uptime(id, window).catch(() => null),
      ]);
      const states = normalizeCurves(raw, 'state')
        .filter((c) => c.field === 'state')
        .flatMap((c) => c.points);
      store.update((s) => ({
        ...s,
        [id]: { states, uptime: normalizeUptime(up), loadedAt: Date.now() },
      }));
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      // On note l'essai pour ne pas reboucler sur une sonde que le hub refuse.
      store.update((s) => ({
        ...s,
        [id]: { states: s[id]?.states ?? [], uptime: s[id]?.uptime ?? {}, loadedAt: Date.now() },
      }));
    }
  }

  /** Relit les sondes dont la bande a plus de 5 min. Les autres attendent. */
  async function refresh(ids: string[], onExpired: () => void) {
    const now = Date.now();
    const current = get(store);
    const due = ids.filter(
      (id) => !inFlight.has(id) && now - (current[id]?.loadedAt ?? 0) >= MAX_AGE_MS,
    );
    for (const id of due) inFlight.add(id);
    let next = 0;
    async function worker() {
      while (next < due.length) {
        const id = due[next++];
        try {
          await readOne(id, onExpired);
        } finally {
          inFlight.delete(id);
        }
      }
    }
    await Promise.all(Array.from({ length: Math.min(PARALLEL, due.length) }, worker));
  }

  return { subscribe: store.subscribe, refresh };
}

export const fleetBeats = create();
