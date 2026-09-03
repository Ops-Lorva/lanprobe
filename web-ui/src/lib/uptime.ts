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
  /**
   * Premier et dernier relevé DÉTERMINÉ de la fenêtre, en secondes epoch.
   *
   * ⚠️ Absents d'un hub antérieur — leur silence ne dit pas « la fenêtre est
   * couverte ». Voir `unmeasuredLabel`.
   */
  first_at?: number | null;
  last_at?: number | null;
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
      first_at: typeof row.first_at === 'number' ? row.first_at : null,
      last_at: typeof row.last_at === 'number' ? row.last_at : null,
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

/**
 * Trois relevés manqués d'affilée décrivent autre chose qu'un retard.
 *
 * ⚠️ Même facteur que `GAP_FACTOR` de `lanprobe-core/src/sla.rs`, et ce n'est
 * pas une coïncidence à préserver au hasard : c'est lui qui garantit que la
 * carte ne compte jamais un bord que le rapport, lui, laisserait passer.
 */
const GAP_FACTOR = 3;

/** Ce qu'on affiche est un dixième de point : en dessous, il n'y a rien à dire. */
const PLUS_PETIT_AFFICHABLE = 0.05;

type Translate = (key: string, values?: Record<string, string | number>) => string;

/**
 * Part de la fenêtre affichée dont on peut AFFIRMER qu'aucun relevé ne parle,
 * ou `null` quand il n'y a rien à dire.
 *
 * 🔴 C'est la mention qui manquait à côté du taux. « 99,5 % » sur six heures
 * dont vingt-huit minutes seulement ont été mesurées se lit « tout va bien
 * depuis six heures » alors qu'il dit « tout allait bien pendant que je
 * regardais ». Le taux reste juste — l'indéterminé sort du dénominateur, c'est
 * la règle — mais seul, il est plausible et faux.
 *
 * 🔴 **Un MINORANT, jamais la couverture fine du rapport.** Ce chemin-ci ne
 * connaît que le premier et le dernier relevé de la fenêtre : il compte les
 * deux BORDS, et rien de ce qui manque entre les deux. `compute_coverage`, qui
 * a les horodatages, compte les bords ET les trous intérieurs. La carte en dit
 * donc toujours MOINS que le rapport, jamais plus, et les deux écrans ne
 * peuvent pas se contredire. D'où le « au moins » du libellé : il est porté
 * par le calcul, pas par une précaution de rédaction.
 *
 * 🔴 **Aucune cadence n'est supposée.** Le seuil au-delà duquel un bord devient
 * un trou est l'intervalle MOYEN OBSERVÉ — `(dernier − premier) / (relevés −
 * 1)` — multiplié par [`GAP_FACTOR`]. Il est toujours ≥ l'intervalle médian
 * dont le rapport se sert, donc toujours plus exigeant : ce que la carte
 * signale, le rapport le signale aussi.
 *
 * ⚠️ `null` quand la fenêtre est couverte : un « 0 % » permanent est du bruit
 * qui finirait par masquer le cas utile. Même règle que `coverageLabel`.
 *
 * ⚠️ `null` aussi quand le hub ne donne pas les bornes, et sous deux relevés :
 * un point isolé ne donne aucun intervalle, donc aucun seuil. On n'en invente
 * pas un — c'est précisément ce que ce calcul refuse.
 */
export function unmeasuredLabel(
  row: TargetUptime | null | undefined,
  fromSecs: number,
  toSecs: number,
  t: Translate = (k) => k,
): string | null {
  if (!row) return null;
  const windowSecs = toSecs - fromSecs + 1;
  if (windowSecs <= 0) return null;
  const { first_at: first, last_at: last } = row;
  if (typeof first !== 'number' || typeof last !== 'number') return null;
  if (row.samples < 2) return null;

  const moyen = Math.max(1, (last - first) / (row.samples - 1));
  const seuil = moyen * GAP_FACTOR;

  // Les bords ne sont ouverts par aucun relevé : ils comptent en entier.
  let gap = 0;
  const avant = first - fromSecs;
  if (avant > seuil) gap += avant;
  const apres = toSecs - last;
  if (apres > seuil) gap += apres;

  const pct = (Math.min(gap, windowSecs) / windowSecs) * 100;
  if (pct < PLUS_PETIT_AFFICHABLE) return null;
  return t('probe.uptime_gap', { pct: pct.toFixed(1) });
}
