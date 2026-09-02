/**
 * Fraîcheur d'une sonde, décidée **dans le navigateur**.
 *
 * ⚠️ Le `status` du hub ne suffit pas, pour deux raisons différentes.
 *
 * D'abord il est figé au moment de la requête : l'horloge partagée vieillit
 * l'écran toutes les 15 s, pas le statut. Un parc laissé ouvert affiche donc
 * un verdict qui a l'âge du dernier chargement.
 *
 * Ensuite et surtout, `online` côté hub tolère trois battements manqués. Un
 * Mac rallumé le matin bat une fois puis se tait ; pendant tout l'intervalle
 * de tolérance, l'interface affirmait « en ligne », en vert, une disponibilité
 * qu'elle ne pouvait pas connaître. Ce n'est pas un mensonge, c'est pire :
 * c'est plausible, donc personne ne le remet en cause.
 *
 * On ne garde donc le vert que dans la cadence de battement — là, on SAIT — et
 * on rapporte le silence le reste du temps. `silentFor` est la valeur utile :
 * « depuis quand » se lit, « en ligne ou pas » se croit.
 */

/**
 * — `online` : battement reçu dans la cadence. Seul état qui affirme.
 * — `silent` : silence sous le seuil. On ne sait pas, on dit depuis quand.
 * — `offline` : silence au-delà du seuil. Là, on conclut.
 */
export type LivenessState = 'online' | 'silent' | 'offline';

export interface Liveness {
  state: LivenessState;
  /**
   * Secondes écoulées depuis le dernier battement.
   *
   * ⚠️ `null` — et non `0` — quand la sonde n'a jamais battu : c'est ce qui
   * permet à l'écran d'écrire « jamais vue » plutôt qu'un âge calculé depuis
   * l'epoch, qui se lirait « hors ligne depuis 56 ans ».
   */
  silentFor: number | null;
}

/** Cadence de battement par défaut du hub — `SETTINGS_DEFAULTS.heartbeat_interval_secs`. */
export const HEARTBEAT_SECS = 60;

/**
 * Seuil au-delà duquel une sonde n'est plus « sans nouvelles » mais absente.
 *
 * Deux heures, comme `OFFLINE_AFTER_SECS` dans `crates/lanprobe-web/src/probe.rs` :
 * les deux valeurs décrivent le même seuil, et l'écran ne doit pas conclure
 * « hors ligne » avant le hub qui envoie les alertes.
 */
export const OFFLINE_AFTER_SECS = 2 * 3600;

export function probeLiveness(
  lastSeen: number | null | undefined,
  atMs: number,
  heartbeatSecs: number = HEARTBEAT_SECS,
  offlineAfterSecs: number = OFFLINE_AFTER_SECS,
): Liveness {
  if (lastSeen == null) return { state: 'offline', silentFor: null };

  // Une sonde dont l'horloge avance sur celle du hub donnerait un âge négatif,
  // donc « il y a -3 min » à l'écran. Elle vient de parler, c'est tout ce
  // qu'on peut en dire.
  const silentFor = Math.max(0, Math.floor(atMs / 1000 - lastSeen));

  if (silentFor < heartbeatSecs) return { state: 'online', silentFor };
  if (silentFor < offlineAfterSecs) return { state: 'silent', silentFor };
  return { state: 'offline', silentFor };
}
