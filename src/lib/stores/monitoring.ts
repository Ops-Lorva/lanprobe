import { writable } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';

export interface PingEntry {
  timestamp: number;
  alive: boolean;
  latency_ms: number | null;
}

export interface HostMonitor {
  ip: string;
  current: { alive: boolean; latency_ms: number | null } | null;
  history: PingEntry[];
}

function createMonitoringStore() {
  const { subscribe, update } = writable<Map<string, HostMonitor>>(new Map());

  // Listener Tauri enregistré une seule fois, au niveau du module → il survit
  // aux changements de page. Sans ça, onDestroy du PingMonitor désabonnait et
  // les ticks en arrière-plan n'arrivaient plus jamais si on quittait la page.
  let initPromise: Promise<void> | null = null;
  let tickUnlisten: UnlistenFn | null = null;

  // Hôtes récemment supprimés : on ignore les ticks « en vol » qui arrivent
  // juste après le stop (le backend peut émettre un dernier tick si le stop
  // tombe pendant un ping déjà lancé), sinon record() recrée l'entrée et
  // l'hôte « réapparaît » dans le monitoring après suppression.
  const tombstones = new Map<string, number>(); // ip -> expiration (ms epoch)
  const TOMBSTONE_MS = 3000;

  const record = (ip: string, alive: boolean, latency_ms: number | null, timestamp: number) => {
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
      host.current = { alive, latency_ms };
      host.history = [...host.history.slice(-59), { timestamp, alive, latency_ms }];
      map.set(ip, host);
      return new Map(map);
    });
  };

  async function init() {
    if (initPromise) return initPromise;
    initPromise = (async () => {
      tickUnlisten = await listen<{ ip: string; alive: boolean; latency_ms: number | null; timestamp: number }>(
        'ping:tick',
        ({ payload: p }) => record(p.ip, p.alive, p.latency_ms, p.timestamp)
      );
      // Hydratation : on récupère l'historique déjà en place côté backend
      // (pour le cas d'un client web qui se connecte alors que des
      // monitorings tournent déjà, ou d'un rechargement de la webview).
      try {
        const snap = await invoke<Record<string, { ip: string; alive: boolean; latency_ms: number | null; timestamp: number }[]>>(
          'cmd_get_monitoring_snapshot'
        );
        if (snap && typeof snap === 'object') {
          update(map => {
            for (const [ip, samples] of Object.entries(snap)) {
              if (!samples || samples.length === 0) continue;
              const history = samples.slice(-60).map(s => ({
                timestamp: s.timestamp,
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
    addHost: (ip: string) => update(map => {
      tombstones.delete(ip); // (ré)ajout explicite → on lève le tombstone
      if (!map.has(ip)) map.set(ip, { ip, current: null, history: [] });
      return new Map(map);
    }),
    removeHost: (ip: string) => update(map => {
      tombstones.set(ip, Date.now() + TOMBSTONE_MS);
      map.delete(ip);
      return new Map(map);
    }),
    record,
    // Pour le hot reload dev seulement — jamais appelé en prod.
    _teardown: () => {
      tickUnlisten?.();
      tickUnlisten = null;
      initPromise = null;
    },
  };
}

export const monitoring = createMonitoringStore();
