import { writable, derived } from 'svelte/store';
import { api, ApiError, type PendingEnroll } from './api';

/**
 * Enrôlements en attente — les emplacements réservés du parc.
 *
 * Deux sources, qui ne se doublonnent pas :
 *
 * 1. **Le hub** (`GET /api/enroll-codes/pending`) fait autorité : c'est lui qui
 *    sait ce qui est encore ouvert, et il sert le code tant qu'il est valable.
 *    C'est ce qui fait qu'un rechargement de page ne perd rien.
 * 2. **Ce navigateur**, qui vient de créer un code, l'affiche sans attendre le
 *    tour suivant — un aller-retour de latence entre le clic et le code serait
 *    lu comme un échec.
 *
 * La mémoire locale ne survit pas à la page, et c'est délibéré : rien n'est
 * écrit dans `localStorage` ni `sessionStorage`. Le hub est déjà la source, y
 * recopier un code d'enrôlement ne ferait qu'en créer une seconde, hors de son
 * contrôle et sans expiration.
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
  /** `null` = code consommé ou expiré : la ligne propose alors son remplacement. */
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
   *
   * ⚠️ Un 403 non plus. `GET /api/enroll-codes/pending` exige `operator` — le
   * code y figure en clair pendant sa validité. Un `viewer` reçoit donc un
   * refus légitime sur cet appel-là, et le traiter comme une session expirée
   * le renvoyait à l'écran de connexion À CHAQUE chargement du parc : le hub
   * était inutilisable pour le seul rôle qui n'a que du parc à regarder. Seul
   * un 401 dit que la session est tombée.
   */
  async function refresh(onExpired: () => void) {
    try {
      const rows = (await api.pendingEnrollCodes()) ?? [];
      update((s) => ({ ...s, server: rows, serverUnavailable: false }));
    } catch (e) {
      if (e instanceof ApiError && e.isExpired) return onExpired();
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

  return { subscribe, refresh, remember, hide };
}

export const enrollments = create();

/**
 * Liste unifiée. Le hub fait autorité sur l'existence d'une attente ; la
 * mémoire locale ne sert qu'à combler un code que le hub n'aurait pas donné —
 * le cas du tout premier affichage, avant le rafraîchissement suivant.
 */
export const pendingRows = derived(enrollments, ($e): PendingRow[] => {
  const byKey = new Map<string, PendingRow>();
  for (const entry of $e.server) {
    const key = pendingKey(entry);
    byKey.set(key, { ...entry, key, code: entry.code ?? $e.local[key]?.code ?? null });
  }
  for (const [key, { entry, code }] of Object.entries($e.local)) {
    if (!byKey.has(key)) byKey.set(key, { ...entry, key, code });
  }
  return [...byKey.values()]
    .filter((r) => !$e.hidden.includes(r.key))
    .sort((a, b) => a.expires_at - b.expires_at);
});
