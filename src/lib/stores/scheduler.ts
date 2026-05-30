import { writable } from 'svelte/store';
import { getConfigStore } from './configStore';

export interface SchedulerConfig {
  speedtest_interval_min: number;
  discovery_interval_min: number;
  discovery_cidr: string;
  portscan_interval_min: number;
  portscan_targets: string[];
}

const DEFAULT_SCHEDULER: SchedulerConfig = {
  speedtest_interval_min: 0,
  discovery_interval_min: 0,
  discovery_cidr: '',
  portscan_interval_min: 0,
  portscan_targets: [],
};

function createSchedulerStore() {
  const { subscribe, set } = writable<SchedulerConfig>({ ...DEFAULT_SCHEDULER, portscan_targets: [] });

  // Idempotent : la config peut désormais être chargée depuis plusieurs
  // composants (Discovery / PortScan / SpeedTest), on ne charge qu'une fois.
  let initPromise: Promise<void> | null = null;
  async function init() {
    if (initPromise) return initPromise;
    initPromise = (async () => {
      const store = await getConfigStore();
      const saved = await store.get<SchedulerConfig>('scheduler');
      if (saved) {
        set({
          ...DEFAULT_SCHEDULER,
          ...saved,
          portscan_targets: Array.isArray(saved.portscan_targets) ? saved.portscan_targets : [],
        });
      }
    })();
    return initPromise;
  }

  async function save(cfg: SchedulerConfig) {
    const store = await getConfigStore();
    await store.set('scheduler', cfg);
    await store.save();
    set({ ...cfg, portscan_targets: [...cfg.portscan_targets] });
  }

  return { subscribe, init, save };
}

export const scheduler = createSchedulerStore();
