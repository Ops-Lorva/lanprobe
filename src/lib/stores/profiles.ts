import { writable } from 'svelte/store';
import { scheduleBackup } from '../hubConfigBackup';
import { getConfigStore } from './configStore';

export interface Profile {
  id: string;
  name: string;
  ip: string;
  subnet: string;
  gateway: string;
  dns_primary: string;
  dns_secondary: string;
  monitor_ips?: string[];
}

const STORE_KEY = 'profiles';

function createProfilesStore() {
  const { subscribe, set, update } = writable<Profile[]>([]);

  async function init() {
    const store = await getConfigStore();
    const saved = await store.get<Profile[]>(STORE_KEY);
    if (saved) set(saved);
  }

  async function persist(profiles: Profile[]) {
    const store = await getConfigStore();
    await store.set(STORE_KEY, profiles);
    await store.save();
    // Le hub garde une copie : une machine réinstallée retrouve ses profils
    // à son ré-enrôlement. Différé et silencieux — voir `hubConfigBackup`.
    scheduleBackup();
  }

  return {
    subscribe,
    init,
    add: (p: Profile) => update(profiles => { const next = [...profiles, p]; persist(next); return next; }),
    edit: (p: Profile) => update(profiles => { const next = profiles.map(x => x.id === p.id ? p : x); persist(next); return next; }),
    remove: (id: string) => update(profiles => { const next = profiles.filter(x => x.id !== id); persist(next); return next; }),
  };
}

export const profiles = createProfilesStore();
