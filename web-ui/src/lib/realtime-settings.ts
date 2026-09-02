/**
 * Les trois réglages du mode temps réel, et la seule règle qui les valide.
 *
 * Ils vivent ensemble parce qu'on les règle ensemble : « combien de temps le
 * mode dure », « quelle fenêtre on regarde pendant », « à quelle cadence la
 * sonde bat pendant ». Séparés, on en réglait un et on cherchait les deux
 * autres — la durée était sur « Général », les deux autres nulle part.
 *
 * ⚠️ La validation est ici, dans un `.ts` testé, et non dans le gabarit :
 * c'est le seul endroit où le plancher de 5 s peut être comparé à celui du
 * hub sans qu'une deuxième implémentation dérive en silence.
 */

/**
 * Plancher de la cadence, en secondes.
 *
 * ⚠️ Ce n'est pas une préférence d'interface : la sonde applique `max(5)`
 * (`crates/lanprobe-server/src/hub.rs`), et le hub refuse en dessous
 * (`settings.rs`, `MIN_HEARTBEAT_INTERVAL_SECS`). Un champ plus permissif
 * afficherait « 2 s » pendant que la sonde bat à 5 — une valeur plausible et
 * fausse, que personne ne remet en cause puisqu'elle vient du formulaire.
 */
export const MIN_HEARTBEAT_SECS = 5;

export interface RealtimeDraft {
  /** Durée du mode avant extinction automatique, en minutes. */
  durationMin: number;
  /** Fenêtre affichée pendant le mode, en minutes. */
  windowMin: number;
  /** Cadence du battement pendant le mode, en secondes. */
  heartbeatSecs: number;
}

/** Défauts du hub (`crates/lanprobe-web/src/settings.rs`). */
export const REALTIME_DEFAULTS: RealtimeDraft = {
  durationMin: 30,
  windowMin: 10,
  heartbeatSecs: 5,
};

/** Champ fautif. L'écran le traduit et y emmène le curseur. */
export type RealtimeIssue = 'duration' | 'window' | 'heartbeat';

/**
 * Premier champ fautif, ou `null`. Un seul à la fois : trois messages
 * simultanés laisseraient chercher lequel corriger d'abord.
 */
export function validateRealtime(draft: RealtimeDraft): RealtimeIssue | null {
  // Une durée nulle armerait un mode déjà expiré : le bouton répondrait
  // « c'est fait » sans que rien ne change sur la sonde.
  if (!Number.isInteger(draft.durationMin) || draft.durationMin < 1) return 'duration';
  // Une fenêtre nulle ne borne aucun axe : le graphe montrerait un instant.
  if (!Number.isInteger(draft.windowMin) || draft.windowMin < 1) return 'window';
  if (!Number.isInteger(draft.heartbeatSecs) || draft.heartbeatSecs < MIN_HEARTBEAT_SECS)
    return 'heartbeat';
  return null;
}
