import { writable, get } from 'svelte/store';
import { api, ApiError, type Probe, type Site } from './api';

interface FleetState {
  sites: Site[];
  probes: Probe[];
  /**
   * Montrer les sites et sondes archivés.
   *
   * ⚠️ Le filtre est appliqué par le hub, pas ici : trier après coup ferait
   * quand même voyager les archives sur le réseau, et le décompte du parc
   * viendrait d'une liste que l'interface a raccourcie elle-même.
   */
  showArchived: boolean;
  /** Filtre serveur : `null` = tout le parc. */
  siteId: string | null;
  /** Premier chargement — c'est le seul cas qui affiche un écran de chargement. */
  loading: boolean;
  /** Rechargement avec des données déjà à l'écran : on garde le rendu. */
  refreshing: boolean;
  error: ApiError | null;
  loadedAt: number | null;
}

const initial: FleetState = {
  sites: [],
  probes: [],
  showArchived: false,
  siteId: null,
  loading: true,
  refreshing: false,
  error: null,
  loadedAt: null,
};

function create() {
  const store = writable<FleetState>({ ...initial });
  const { subscribe, update } = store;

  /**
   * `onExpired` remonte les 401 à l'application : c'est elle qui décide de
   * renvoyer vers l'écran de connexion. Un store qui redirigerait lui-même
   * rendrait le composant impossible à tester isolément.
   */
  async function load(onExpired: () => void, opts: { quiet?: boolean } = {}) {
    const current = get(store);
    update((s) => ({
      ...s,
      loading: opts.quiet ? s.loading : s.probes.length === 0,
      refreshing: opts.quiet || s.probes.length > 0,
      error: null,
    }));
    try {
      const [sites, probes] = await Promise.all([
        api.sites(current.showArchived),
        api.probes(current.siteId ?? undefined, current.showArchived),
      ]);
      update((s) => ({
        ...s,
        sites,
        probes,
        loading: false,
        refreshing: false,
        loadedAt: Date.now(),
      }));
    } catch (e) {
      const err = e instanceof ApiError ? e : new ApiError(0, String(e));
      // Seul un 401 renvoie à la connexion. Un 403 est un refus de rôle : s'y
      // reconnecter avec les mêmes identifiants donnerait le même refus, alors
      // qu'affiché comme erreur il nomme au moins la règle appliquée.
      if (err.isExpired) {
        update((s) => ({ ...s, loading: false, refreshing: false }));
        onExpired();
        return;
      }
      update((s) => ({ ...s, loading: false, refreshing: false, error: err }));
    }
  }

  function selectSite(siteId: string | null, onExpired: () => void) {
    update((s) => ({ ...s, siteId, probes: [], loading: true }));
    return load(onExpired);
  }

  /** Mise à jour locale après un PATCH réussi : évite un aller-retour complet. */
  function patchLocal(probe: Probe) {
    update((s) => ({
      ...s,
      probes: s.probes.map((p) => (p.probe_id === probe.probe_id ? { ...p, ...probe } : p)),
    }));
  }

  function dropLocal(probeId: string) {
    update((s) => ({ ...s, probes: s.probes.filter((p) => p.probe_id !== probeId) }));
  }

  function renameSiteLocal(siteId: string, name: string) {
    update((s) => ({
      ...s,
      sites: s.sites.map((x) => (x.site_id === siteId ? { ...x, name } : x)),
      probes: s.probes.map((p) => (p.site_id === siteId ? { ...p, site: name } : p)),
    }));
  }

  /**
   * Bascule l'affichage des archives et recharge.
   *
   * Le rechargement est immédiat et non « au prochain rafraîchissement » :
   * cocher une case qui ne change rien pendant trente secondes se lit comme
   * une case cassée.
   */
  function toggleArchived(onExpired: () => void) {
    update((s) => ({ ...s, showArchived: !s.showArchived }));
    void load(onExpired, { quiet: true });
  }

  return {
    subscribe,
    load,
    selectSite,
    patchLocal,
    dropLocal,
    renameSiteLocal,
    toggleArchived,
  };
}

export const fleet = create();
