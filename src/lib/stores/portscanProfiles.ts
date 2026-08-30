import { writable } from 'svelte/store';
import { scheduleBackup } from '../hubConfigBackup';
import { getConfigStore } from './configStore';

export interface PortScanProfile {
  id: string;
  name: string;
  tcp_ports: number[];
  udp_ports: number[];
  builtin?: boolean;
}

const STORE_KEY = 'portscan_profiles';
const ACTIVE_KEY = 'portscan_active_profile';

// Profils fournis par défaut. Non persistés (recalculés à chaque init) —
// on ne veut pas qu'une mise à jour de LanProbe laisse en base l'ancienne
// liste de ports d'un preset.
export const BUILTIN_PROFILES: PortScanProfile[] = [
  {
    id: 'builtin:common',
    name: 'Common',
    tcp_ports: [21, 22, 23, 25, 53, 80, 110, 143, 443, 445, 3306, 3389, 5432, 5900, 8080, 8443],
    udp_ports: [53, 123, 137, 161, 1900, 5353],
    builtin: true,
  },
  {
    id: 'builtin:web',
    name: 'Web',
    tcp_ports: [80, 443, 8000, 8008, 8080, 8081, 8088, 8181, 8443, 8888, 3000, 5000, 9000],
    udp_ports: [],
    builtin: true,
  },
  {
    id: 'builtin:db',
    name: 'Databases',
    tcp_ports: [1433, 1521, 3306, 5432, 5984, 6379, 7000, 9042, 9200, 9300, 11211, 27017, 50000],
    udp_ports: [1434],
    builtin: true,
  },
  {
    id: 'builtin:remote',
    name: 'Remote access',
    tcp_ports: [22, 23, 2222, 3389, 5900, 5901, 5902, 5938, 6000],
    udp_ports: [],
    builtin: true,
  },
  {
    id: 'builtin:full',
    name: 'Full (extended)',
    tcp_ports: [
      21, 22, 23, 25, 53, 80, 110, 111, 135, 139, 143, 389, 443, 445, 465, 514, 587,
      631, 636, 873, 993, 995, 1080, 1194, 1433, 1521, 2049, 2222, 2375, 2376,
      3000, 3128, 3306, 3389, 5000, 5060, 5222, 5432, 5672, 5900, 5984, 6379, 6443,
      7000, 8000, 8008, 8080, 8086, 8088, 8181, 8443, 8883, 9000, 9092, 9200, 9300,
      11211, 27017, 50000,
    ],
    udp_ports: [53, 67, 68, 69, 123, 137, 161, 500, 514, 1900, 4500, 5353],
    builtin: true,
  },
];

function createPortScanProfilesStore() {
  const { subscribe, set, update } = writable<PortScanProfile[]>(BUILTIN_PROFILES);
  const active = writable<string>('builtin:common');

  async function init() {
    const store = await getConfigStore();
    const saved = await store.get<PortScanProfile[]>(STORE_KEY);
    const custom = (saved ?? []).filter(p => !p.builtin && !p.id.startsWith('builtin:'));
    set([...BUILTIN_PROFILES, ...custom]);
    const savedActive = await store.get<string>(ACTIVE_KEY);
    if (savedActive) active.set(savedActive);
  }

  async function persist(profiles: PortScanProfile[]) {
    const store = await getConfigStore();
    const custom = profiles.filter(p => !p.builtin && !p.id.startsWith('builtin:'));
    await store.set(STORE_KEY, custom);
    await store.save();
    scheduleBackup();
  }

  async function persistActive(id: string) {
    const store = await getConfigStore();
    await store.set(ACTIVE_KEY, id);
    await store.save();
  }

  return {
    subscribe,
    init,
    active: { subscribe: active.subscribe },
    setActive: (id: string) => { active.set(id); persistActive(id); },
    add: (p: PortScanProfile) => update(profiles => {
      const next = [...profiles, p];
      persist(next);
      return next;
    }),
    edit: (p: PortScanProfile) => update(profiles => {
      const next = profiles.map(x => x.id === p.id ? p : x);
      persist(next);
      return next;
    }),
    remove: (id: string) => update(profiles => {
      const next = profiles.filter(x => x.id !== id);
      persist(next);
      return next;
    }),
  };
}

export const portscanProfiles = createPortScanProfilesStore();
