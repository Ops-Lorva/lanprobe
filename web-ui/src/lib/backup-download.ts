/**
 * Comment une archive sort du hub : méthode et adresse.
 *
 * 🔴 **Les deux chemins passent par le MÊME formulaire, soumis à la main.**
 * Le téléchargement sans mot de passe — le cas par défaut — ne produisait
 * aucun fichier : il passait par un lien du gabarit,
 * `<a href={download ? urlDe(download) : '#'}>`, dont le gestionnaire de clic
 * remettait `download` à `null` pour fermer le dialogue. Svelte vide sa file
 * de mises à jour au point de contrôle des micro-tâches qui suit un
 * gestionnaire d'événement (`Batch.ensure` → `queue_micro_task`), c'est-à-dire
 * AVANT que le navigateur n'exécute le comportement d'activation du lien et
 * n'y lise `href` : l'attribut valait déjà « # », et il n'y avait rien à
 * télécharger. Le chemin protégé n'en souffrait pas, mais par accident — il
 * soumettait son formulaire AVANT de changer d'état.
 *
 * Une soumission de formulaire programmée ne dépend de rien de ce qui se passe
 * après : l'adresse et la méthode sont lues au moment du `submit()`. Il n'y a
 * plus de course à gagner.
 *
 * ⚠️ `GET` pour le cas simple et `POST` pour le cas scellé, et non `POST` pour
 * les deux : un téléchargement est une lecture, et c'est l'adresse qu'on colle
 * dans un terminal. Le hub sert les deux méthodes sur une seule route.
 *
 * ⚠️ **Le mot de passe ne va jamais dans l'adresse.** `access_log` du hub
 * journalise la requête avec sa chaîne de requête : un `?password=…` finirait
 * en clair dans les journaux du conteneur. Il voyage dans le corps, donc dans
 * un champ du formulaire — ce qui impose `POST` pour ce chemin-là.
 *
 * ⚠️ Et cette règle vit dans un `.ts` plutôt que dans les attributs du
 * gabarit : écrite dans le gabarit, elle n'était couverte par rien, et c'est
 * exactement là que le défaut a vécu.
 */

import { backupDownloadUrl } from './api';

export interface DownloadRequest {
  method: 'GET' | 'POST';
  action: string;
}

/** La requête qui sort l'archive `file`, scellée ou non. */
export function downloadRequest(file: string, sealed: boolean): DownloadRequest {
  return { method: sealed ? 'POST' : 'GET', action: backupDownloadUrl(file) };
}
