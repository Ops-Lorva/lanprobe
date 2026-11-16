/**
 * Un tour de lecture de la fiche, attaché à la sonde qui l'a lancé.
 *
 * ⚠️ `id` suit la route : il change quand on passe à une autre sonde, et ne
 * vaut plus rien une fois la fiche quittée. Une lecture qui le relit APRÈS un
 * `await` interroge donc une autre sonde, ou `/api/probes/undefined/…` — vu
 * en prod sur `/uptime` et `/public-ips`, lancées derrière des courbes qui
 * prennent plusieurs secondes. On fige l'identifiant au départ, et on demande
 * au retour si le tour compte encore.
 */
export interface ProbeCycle {
  /** L'identifiant lu au départ du tour. */
  id: string | undefined;
  /** Vrai quand la fiche montre une autre sonde, ou plus aucune. */
  stale: () => boolean;
}

export function probeCycle(current: () => string | undefined): ProbeCycle {
  const id = current();
  return { id, stale: () => !id || current() !== id };
}
