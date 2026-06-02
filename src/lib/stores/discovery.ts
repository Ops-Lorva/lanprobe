import { writable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';

export interface DiscoveredHost {
  ip: string;
  hostname: string | null;
  mac: string | null;
  vendor: string | null;
  latency_ms: number | null;
}

interface DiscoveryState {
  results: Map<string, DiscoveredHost>;
  scanning: boolean;
  cidr: string;
  error: string;
}

function createDiscoveryStore() {
  const { subscribe, update, set } = writable<DiscoveryState>({
    results: new Map(),
    scanning: false,
    cidr: '',
    error: '',
  });

  return {
    subscribe,
    addHost(host: DiscoveredHost) {
      update(s => {
        const results = new Map(s.results);
        const existing = results.get(host.ip);
        // Merge non-destructif : on ne perd jamais une info déjà connue
        // (mac vu via ARP, hostname résolu, latence mesurée…).
        results.set(host.ip, {
          ip: host.ip,
          hostname: host.hostname ?? existing?.hostname ?? null,
          mac: host.mac ?? existing?.mac ?? null,
          vendor: host.vendor ?? existing?.vendor ?? null,
          latency_ms: host.latency_ms ?? existing?.latency_ms ?? null,
        });
        return { ...s, results };
      });
    },
    updateLatency(ip: string, latency_ms: number) {
      update(s => {
        const existing = s.results.get(ip);
        if (!existing) return s;
        const results = new Map(s.results);
        results.set(ip, { ...existing, latency_ms });
        return { ...s, results };
      });
    },
    updateMac(ip: string, mac: string, vendor: string | null = null) {
      update(s => {
        const existing = s.results.get(ip);
        const results = new Map(s.results);
        if (existing) {
          results.set(ip, { ...existing, mac, vendor: vendor ?? existing.vendor ?? null });
        } else {
          results.set(ip, { ip, hostname: null, mac, vendor, latency_ms: null });
        }
        return { ...s, results };
      });
    },
    setScanning(scanning: boolean) { update(s => ({ ...s, scanning })); },
    setCidr(cidr: string)          { update(s => ({ ...s, cidr })); },
    setError(error: string)        { update(s => ({ ...s, error })); },
    clear() { update(s => ({ ...s, results: new Map(), error: '' })); },
    reset() { set({ results: new Map(), scanning: false, cidr: '', error: '' }); },
    async hydrate() {
      try {
        const hosts = await invoke<DiscoveredHost[]>('cmd_get_discovery_snapshot');
        if (!hosts || hosts.length === 0) return;
        update(s => {
          const results = new Map(s.results);
          for (const h of hosts) {
            const existing = results.get(h.ip);
            results.set(h.ip, {
              ip: h.ip,
              hostname: h.hostname ?? existing?.hostname ?? null,
              mac: h.mac ?? existing?.mac ?? null,
              vendor: h.vendor ?? existing?.vendor ?? null,
              latency_ms: h.latency_ms ?? existing?.latency_ms ?? null,
            });
          }
          return { ...s, results };
        });
      } catch {
        // pas de backend dispo (préview), on ignore
      }
    },
  };
}

export const discoveryStore = createDiscoveryStore();
