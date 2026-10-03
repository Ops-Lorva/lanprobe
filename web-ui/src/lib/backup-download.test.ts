/**
 * Comment part une archive : méthode et adresse, selon le choix de protection.
 *
 * Le défaut qui motive ce fichier : le téléchargement SANS mot de passe — le
 * cas par défaut — ne produisait aucun fichier. Il passait par un
 * `<a href={download ? … : '#'}>` dont le gestionnaire de clic remettait
 * `download` à `null` : Svelte vidait sa file de mises à jour au point de
 * contrôle des micro-tâches qui suit un gestionnaire, donc AVANT que le
 * navigateur ne lise `href` pour suivre le lien, et le lien valait déjà « # ».
 * Le chemin protégé y échappait par accident, en soumettant son formulaire
 * avant de changer d'état.
 */

import { describe, expect, it } from 'vitest';
import { downloadRequest } from './backup-download';

const FICHIER = 'lanprobe_backup_v1.0.0_2026.08.28_14.30.00.zip';

describe('downloadRequest', () => {
  it('télécharge en GET quand il n’y a pas de mot de passe', () => {
    // Un téléchargement est une lecture : le cas par défaut reste un GET,
    // c'est aussi l'adresse qu'on colle dans un terminal.
    expect(downloadRequest(FICHIER, false)).toEqual({
      method: 'GET',
      action: `/api/backups/${FICHIER}`,
    });
  });

  it('scelle en POST, sur la MÊME adresse', () => {
    // Deux méthodes et une seule route, côté hub comme ici : le mot de passe
    // voyage dans le corps du formulaire.
    expect(downloadRequest(FICHIER, true)).toEqual({
      method: 'POST',
      action: `/api/backups/${FICHIER}`,
    });
  });

  it('ne met JAMAIS le mot de passe dans l’adresse', () => {
    // ⚠️ `access_log` du hub journalise la requête avec sa chaîne de requête :
    // un `?password=…` finirait en clair dans les journaux du conteneur.
    for (const sealed of [false, true]) {
      const { action } = downloadRequest(FICHIER, sealed);
      expect(action).not.toContain('password');
      expect(action).not.toContain('?');
    }
  });

  it('encode le nom de fichier', () => {
    // La garde de chemin du hub n'accepte qu'un nom nu, mais c'est elle qui
    // doit refuser — pas l'URL qui doit se casser en route.
    expect(downloadRequest('archive bizarre#1.zip', false).action).toBe(
      '/api/backups/archive%20bizarre%231.zip',
    );
  });
});
