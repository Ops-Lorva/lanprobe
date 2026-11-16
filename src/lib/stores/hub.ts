import { writable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import type { HubStatus } from '../status';

/**
 * L'état du rattachement au hub, lu par la barre latérale, le tableau de bord
 * et les réglages. Un seul relevé toutes les 20 s plutôt qu'un par écran.
 *
 * Le rattachement est optionnel : une erreur ici (backend sans hub) laisse
 * l'état « non rattachée », sans rien signaler.
 */
const OFF: HubStatus = {
  enrolled: false, url: '', name: '', site: '', state: 'off',
  buffered_points: 0, last_error: null, refused_status: null, last_beat_at: null,
};

function createHubStore() {
  const { subscribe, set } = writable<HubStatus>(OFF);
  let timer: ReturnType<typeof setInterval> | null = null;
  let users = 0;

  async function refresh() {
    try {
      const h = await invoke<HubStatus | null>('cmd_hub_status');
      set(h ?? OFF);
    } catch {
      /* optionnel */
    }
  }

  /** À appeler au montage ; renvoie la fonction de démontage. */
  function watch(): () => void {
    users++;
    if (!timer) {
      refresh();
      timer = setInterval(refresh, 20000);
    }
    return () => {
      users--;
      if (users === 0 && timer) { clearInterval(timer); timer = null; }
    };
  }

  return { subscribe, refresh, watch };
}

export const hubStatus = createHubStore();
