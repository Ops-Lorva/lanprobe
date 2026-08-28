import { writable, derived, get } from 'svelte/store';
import { api, ApiError, type PendingEnroll } from './api';

/**
 * Enrôlements en attente — les emplacements réservés du parc.
 *
 * Deux sources, et c'est structurel :
 *
 * 1. **Le hub** (`GET /api/enroll-codes/pending`) sait quels codes sont encore
 *    ouverts, mais **jamais leur valeur** : le code est haché en base, il
 *    n'existe en clair qu'à l'instant de sa création. Un code lisible en base
 *    donnerait à qui la vole de quoi enrôler une sonde dans le parc.
 * 2. **Ce navigateur**, qui vient de le créer, l'a en mémoire.
 *
 * D'où la règle : on affiche le code tant qu'on est sur la page qui l'a
 * demandé ; après un rechargement, la ligne subsiste — le hub la connaît — mais
 * le code est perdu et se remplace d'un clic. On ne l'écrit **nulle part** :
 * ni `localStorage`, ni `sessionStorage`. Le contourner reviendrait à recréer
 * en clair, côté client, exactement ce que le hub refuse de stocker.
 */
export type { PendingEnroll };

/** Identité d'une attente, dérivée de ses champs : l'API n'en fournit pas. */
export function pendingKey(p: {
  site_id: string;
  probe_id: string | null;
  expires_at: number;
}): string {
  return `${p.site_id}|${p.probe_id ?? ''}|${p.expires_at}`;
}

export interface PendingRow extends PendingEnroll {
  key: string;
  /** `null` = créée avant ce chargement de page : la valeur n'est plus disponible. */
  code: string | null;
}

interface EnrollState {
  server: PendingEnroll[];
  /** Créées dans cette page — seules à porter le code en clair. */
  local: Record<string, { entry: PendingEnroll; code: string }>;
  /** Masquées à la main : le code reste valable, on ne veut simplement plus le voir. */
  hidden: string[];
  /** Vrai quand le hub ne sert pas encore la route : on ne montre alors que le local. */
  serverUnavailable: boolean;
}

const initial: EnrollState = { server: [], local: {}, hidden: [], serverUnavailable: false };

function create() {
  const store = writable<EnrollState>({ ...initial });
  const { subscribe, update } = store;

  /**
   * Un hub antérieur à cette route répond 404. Ce n'est pas une panne : les
   * attentes créées ici restent affichées, elles ne survivront simplement pas
   * au rechargement. Faire échouer le parc entier pour ça serait disproportionné.
   */
  async function refresh(onExpired: () => void) {
    try {
      const rows = (await api.pendingEnrollCodes()) ?? [];
      update((s) => ({ ...s, server: rows, serverUnavailable: false }));
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      update((s) => ({ ...s, server: [], serverUnavailable: true }));
    }
  }

  function remember(entry: PendingEnroll, code: string) {
    const key = pendingKey(entry);
    update((s) => ({
      ...s,
      local: { ...s.local, [key]: { entry, code } },
      hidden: s.hidden.filter((k) => k !== key),
    }));
  }

  /** Retire la ligne de l'écran. Ne révoque rien : le code court jusqu'au bout. */
  function hide(key: string) {
    update((s) => {
      const local = { ...s.local };
      delete local[key];
      return { ...s, local, hidden: s.hidden.includes(key) ? s.hidden : [...s.hidden, key] };
    });
  }

  return { subscribe, refresh, remember, hide, snapshot: () => get(store) };
}

export const enrollments = create();

/**
 * Liste unifiée. La mémoire locale prime sur le hub pour la même attente :
 * c'est la seule des deux qui porte le code.
 */
export const pendingRows = derived(enrollments, ($e): PendingRow[] => {
  const byKey = new Map<string, PendingRow>();
  for (const entry of $e.server) {
    const key = pendingKey(entry);
    byKey.set(key, { ...entry, key, code: null });
  }
  for (const [key, { entry, code }] of Object.entries($e.local)) {
    byKey.set(key, { ...entry, key, code });
  }
  return [...byKey.values()]
    .filter((r) => !$e.hidden.includes(r.key))
    .sort((a, b) => a.expires_at - b.expires_at);
});
