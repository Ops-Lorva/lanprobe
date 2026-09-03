/**
 * Rétention des **fichiers** de rapport (contrat § 7 et § 23).
 *
 * 🔴 **La purge ne touche que le fichier.** L'entrée d'historique — qui a
 * demandé quoi, quand — n'est jamais purgée. Un rapport se régénère ; la trace
 * de son extraction, non. C'est ce que la ligne de réglage doit dire, sans
 * quoi on croit effacer une trace qu'on garde, ou l'inverse.
 *
 * La règle vit ici, dans un `.ts` testé, et non dans le gabarit : c'est le
 * seul endroit où le plancher peut être comparé à celui du hub sans qu'une
 * deuxième implémentation dérive en silence.
 */

/** Défaut du hub (`REPORT_DAYS_DEFAULT`, `crates/lanprobe-web/src/reports.rs`). */
export const REPORT_DAYS_DEFAULT = 7;

/**
 * Plancher, en jours.
 *
 * 🔴 **`0` n'est pas « illimité » ici : il est refusé.** `retention_days` et
 * `inventory_days` valent `0` = illimité parce qu'y toucher efface des mesures
 * qu'on ne peut pas refaire ; un rapport, lui, se régénère. Et un volume qui
 * accumule les classeurs SLA de tous les clients pendant deux ans est le tas le
 * plus sensible que ce hub sache produire.
 */
export const REPORT_DAYS_MIN = 1;

/**
 * Vrai si la valeur est un nombre de jours que le hub acceptera.
 *
 * Le hub refuse en `400` avec un message qui dit pourquoi ; on le dit ici
 * plutôt que de laisser partir la requête, pour nommer le champ fautif et
 * emmener la personne dessus.
 */
export function isValidReportDays(days: number): boolean {
  return Number.isInteger(days) && days >= REPORT_DAYS_MIN;
}
