/**
 * Qui a fait la ligne d'audit (contrat § 11).
 *
 * 🔴 **« Le hub l'a fait » et « on ne sait pas qui c'est » sont deux choses.**
 * Les deux s'écrivent sans acteur, et l'écran les affichait toutes les deux
 * « anonyme » — le mot d'une tentative de connexion sur un compte inconnu, donc
 * d'un humain qu'on n'a pas pu identifier. Une sauvegarde planifiée, elle, est
 * faite par le hub : chercher qui s'est connecté pour un geste que personne n'a
 * demandé est une perte de temps, et au pire une fausse alerte de sécurité.
 */

export type AuditActor =
  | { kind: 'account'; name: string }
  | { kind: 'system' }
  | { kind: 'unknown' };

/** La marque que cette version sait lire. */
const SYSTEM = 'system';

/**
 * ⚠️ Une marque que cette version ne connaît pas se lit **inconnue**, jamais
 * brute et surtout jamais « système » : ce serait affirmer que le hub a fait
 * quelque chose dont on ne sait rien. Même règle qu'à l'origine d'une commande
 * (§ 26), et pour la même raison — un hub plus récent, un onglet resté ouvert
 * pendant une mise à jour.
 */
export function auditActor(entry: {
  actor: string | null;
  actor_kind?: string | null;
}): AuditActor {
  const name = entry.actor?.trim();
  // Le compte est le fait le plus fort : une marque qui le contredirait ne doit
  // pas effacer le nom de qui a agi.
  if (name) return { kind: 'account', name };
  return entry.actor_kind === SYSTEM ? { kind: 'system' } : { kind: 'unknown' };
}
