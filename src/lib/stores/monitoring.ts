import { writable } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';

export interface PingEntry {
  timestamp: number;
  /** Départ du relevé à la milliseconde ; absent des sondes plus anciennes. */
  timestamp_ms?: number;
  alive: boolean;
  latency_ms: number | null;
}

export interface HostMonitor {
  ip: string;
  current: { alive: boolean; latency_ms: number | null } | null;
  history: PingEntry[];
}

const HISTORY_MAX = 60;
const departure = (e: PingEntry) => e.timestamp_ms ?? e.timestamp * 1000;

/**
 * Ajoute un relevé, rangé par DÉPART et non par arrivée.
 *
 * ⚠️ Plusieurs relevés sont en vol par cible : un relevé muet (~2,5 s) finit
 * après celui lancé une seconde plus tard. Pris comme « dernier arrivé », il
 * repeignait la ligne en rouge juste après le vert — l'état clignotait pendant
 * une panne partielle. `current` est le plus récent par départ.
 */
export function withSample(host: HostMonitor, sample: PingEntry): HostMonitor {
  const history = [...host.history];
  let at = history.length;
  while (at > 0 && departure(history[at - 1]) > departure(sample)) at--;
  history.splice(at, 0, sample);
  const kept = history.slice(-HISTORY_MAX);
  const last = kept[kept.length - 1];
  return { ...host, history: kept, current: { alive: last.alive, latency_ms: last.latency_ms } };
}

function createMonitoringStore() {
  const { subscribe, update } = writable<Map<string, HostMonitor>>(new Map());

  // Listener Tauri enregistré une seule fois, au niveau du module → il survit
  // aux changements de page. Sans ça, onDestroy du PingMonitor désabonnait et
  // les ticks en arrière-plan n'arrivaient plus jamais si on quittait la page.
  let initPromise: Promise<void> | null = null;
  let tickUnlisten: UnlistenFn | null = null;
  let startedUnlisten: UnlistenFn | null = null;
  let stoppedUnlisten: UnlistenFn | null = null;

  // Hôtes récemment supprimés : on ignore les ticks « en vol » qui arrivent
  // juste après le stop (le backend peut émettre un dernier tick si le stop
  // tombe pendant un ping déjà lancé), sinon record() recrée l'entrée et
  // l'hôte « réapparaît » dans le monitoring après suppression.
  const tombstones = new Map<string, number>(); // ip -> expiration (ms epoch)
  const TOMBSTONE_MS = 3000;

  const record = (ip: string, alive: boolean, latency_ms: number | null, timestamp: number, timestamp_ms?: number) => {
    const until = tombstones.get(ip);
    if (until !== undefined) {
      if (Date.now() < until) return;      // tick tardif → ignoré
      tombstones.delete(ip);               // expiré → on nettoie
    }
    update(map => {
      // Auto-enregistrement : quand un tick arrive pour une IP inconnue
      // (cas classique du client web qui se connecte alors qu'un monitoring
      // a déjà été lancé côté desktop), on crée l'entrée à la volée au lieu
      // de dropper l'échantillon — sans ça le web ne verrait jamais les
      // monitorings initiés depuis la machine hôte.
      const host = map.get(ip) ?? { ip, current: null, history: [] as PingEntry[] };
      map.set(ip, withSample(host, { timestamp, timestamp_ms, alive, latency_ms }));
      return new Map(map);
    });
  };

  const addHost = (ip: string) => update(map => {
    tombstones.delete(ip); // (ré)ajout explicite → on lève le tombstone
    if (!map.has(ip)) map.set(ip, { ip, current: null, history: [] });
    return new Map(map);
  });

  const removeHost = (ip: string) => update(map => {
    tombstones.set(ip, Date.now() + TOMBSTONE_MS);
    map.delete(ip);
    return new Map(map);
  });

  async function init() {
    if (initPromise) return initPromise;
    initPromise = (async () => {
      tickUnlisten = await listen<{ ip: string; alive: boolean; latency_ms: number | null; timestamp: number; timestamp_ms?: number }>(
        'ping:tick',
        ({ payload: p }) => record(p.ip, p.alive, p.latency_ms, p.timestamp, p.timestamp_ms)
      );
      // 🔴 Les surveillances décidées AILLEURS — depuis le hub, donc depuis le
      // téléphone. Sans ces deux écoutes, une cible retirée restait affichée
      // avec ses derniers chiffres jusqu'au redémarrage de l'app : la sonde ne
      // la pinguait plus, l'écran disait encore le contraire. Constaté le
      // 29/09. Une cible ajoutée, elle, n'apparaissait pas du tout.
      startedUnlisten = await listen<{ ip: string }>(
        'monitor:started',
        ({ payload: p }) => addHost(p.ip)
      );
      stoppedUnlisten = await listen<{ ip: string }>(
        'monitor:stopped',
        ({ payload: p }) => removeHost(p.ip)
      );
      // Hydratation : on récupère l'historique déjà en place côté backend
      // (pour le cas d'un client web qui se connecte alors que des
      // monitorings tournent déjà, ou d'un rechargement de la webview).
      try {
        const snap = await invoke<Record<string, { ip: string; alive: boolean; latency_ms: number | null; timestamp: number; timestamp_ms?: number }[]>>(
          'cmd_get_monitoring_snapshot'
        );
        if (snap && typeof snap === 'object') {
          update(map => {
            for (const [ip, samples] of Object.entries(snap)) {
              if (!samples || samples.length === 0) continue;
              const history = samples.slice(-60).map(s => ({
                timestamp: s.timestamp,
                timestamp_ms: s.timestamp_ms,
                alive: s.alive,
                latency_ms: s.latency_ms,
              }));
              const last = samples[samples.length - 1];
              map.set(ip, {
                ip,
                current: { alive: last.alive, latency_ms: last.latency_ms },
                history,
              });
            }
            return new Map(map);
          });
        }
      } catch {
        // pas de backend dispo, ignore
      }
    })();
    return initPromise;
  }

  return {
    subscribe,
    init,
    addHost,
    removeHost,
    record,
    // Pour le hot reload dev seulement — jamais appelé en prod.
    _teardown: () => {
      tickUnlisten?.();
      startedUnlisten?.();
      stoppedUnlisten?.();
      tickUnlisten = null;
      startedUnlisten = null;
      stoppedUnlisten = null;
      initPromise = null;
    },
  };
}

export const monitoring = createMonitoringStore();
