import { writable } from 'svelte/store';

export type Theme = 'system' | 'dark' | 'light';

const KEY = 'lanprobe.hub.theme';

function read(): Theme {
  try {
    const v = localStorage.getItem(KEY);
    if (v === 'dark' || v === 'light' || v === 'system') return v;
  } catch {
    /* stockage indisponible (navigation privée stricte) : on reste en système */
  }
  return 'system';
}

/**
 * Même mécanique que l'app desktop (`src/lib/stores/settings.ts`) : l'attribut
 * `data-theme` sur <html> bascule les tokens de `app.css`. La seule différence
 * est le support de persistance — localStorage ici, store Tauri là-bas.
 */
export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  root.removeAttribute('data-theme');
  if (theme !== 'system') root.setAttribute('data-theme', theme);
}

function create() {
  const { subscribe, set } = writable<Theme>(read());
  return {
    subscribe,
    set(theme: Theme) {
      try {
        localStorage.setItem(KEY, theme);
      } catch {
        /* non bloquant */
      }
      applyTheme(theme);
      set(theme);
    },
    init() {
      applyTheme(read());
    },
  };
}

export const theme = create();
