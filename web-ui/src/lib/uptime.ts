/**
 * Taux de disponibilité par cible, lu sur `GET /api/probes/{id}/uptime`.
 *
 * ⚠️ Route SÉPARÉE de `/sla`, et ce n'est pas un doublon : `/sla` rend des
 * relevés bruts et la fiche se rafraîchit toutes les 3 s. Le hub agrège donc
 * côté Influx et ne renvoie qu'une ligne par cible. Le calcul, lui, est le
 * même — `crates/lanprobe-web/src/web.rs` tient sous test l'égalité entre ce
 * chemin agrégé et `compute_sla`.
 */

/** Ce que le hub dit d'une cible sur la fenêtre affichée. */
export interface TargetUptime {
  /**
   * ⚠️ `null` = aucun relevé DÉTERMINÉ sur la période : on ne sait pas.
   *
   * Ni `0`, qui se lirait comme une panne totale, ni une valeur de
   * remplacement. Un `?? 0` sur ce chemin annulerait la règle tenue partout
   * ailleurs depuis ce matin.
   */
  uptime_pct: number | null;
  /** Relevés portant un verdict — le dénominateur du taux. */
  samples: number;
}

/**
 * Indexe la réponse par adresse.
 *
 * Une forme inattendue rend un objet vide plutôt que de lever : l'indicateur
 * disparaît, le reste de la fiche continue de s'afficher.
 */
export function normalizeUptime(payload: unknown): Record<string, TargetUptime> {
  const out: Record<string, TargetUptime> = {};
  if (!payload || typeof payload !== 'object') return out;
  const targets = (payload as Record<string, unknown>).targets;
  if (!Array.isArray(targets)) return out;

  for (const raw of targets) {
    if (!raw || typeof raw !== 'object') continue;
    const row = raw as Record<string, unknown>;
    if (typeof row.ip !== 'string') continue;
    out[row.ip] = {
      uptime_pct: typeof row.uptime_pct === 'number' ? row.uptime_pct : null,
      samples: typeof row.samples === 'number' ? row.samples : 0,
    };
  }
  return out;
}

/**
 * Ce qu'on sait d'une cible, ou `null` si le hub n'en dit rien.
 *
 * ⚠️ Trois états, jamais deux : un taux, « indéterminé » (la cible est connue
 * mais aucun relevé ne porte de verdict), et « on ne sait rien » (le hub n'a
 * pas de ligne — une cible qu'on vient d'ajouter). Les confondre ferait
 * afficher un verdict sur une cible qui n'a pas encore été mesurée.
 */
export function uptimeOf(
  map: Record<string, TargetUptime>,
  ip: string,
): { pct: number | null; samples: number } | null {
  const row = map[ip];
  if (!row) return null;
  return { pct: row.uptime_pct, samples: row.samples };
}
