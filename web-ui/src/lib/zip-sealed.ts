/**
 * « Cette archive est-elle protégée par un mot de passe ? », lu dans le
 * navigateur avant tout envoi.
 *
 * ## Pourquoi côté client
 *
 * Le hub sait répondre — il renvoie `password_required` — mais il ne le sait
 * qu'**après** avoir reçu l'archive. Une sauvegarde avec ses séries fait
 * plusieurs centaines de Mo : découvrir au bout de l'envoi qu'il fallait un
 * mot de passe obligerait à tout renvoyer. On lit donc les premiers octets du
 * fichier choisi, et on demande le mot de passe avant de téléverser quoi que
 * ce soit. Le refus du hub reste le filet : l'API est honnête même sans cet
 * écran.
 *
 * ## Ce qu'on lit
 *
 * Un ZIP commence par l'en-tête de sa première entrée locale :
 *
 * ```text
 * octets 0-3   signature  50 4B 03 04
 * octets 4-5   version nécessaire pour extraire
 * octets 6-7   drapeau général, petit-boutien — le bit 0 dit « chiffrée »
 * ```
 *
 * ⚠️ **Le bit 0 seulement.** Le bit 3 (descripteur de données) et le bit 11
 * (nom en UTF-8) sont posés par à peu près tout écrivain de ZIP, y compris
 * celui du hub : les confondre avec le chiffrement réclamerait un mot de passe
 * pour une archive en clair, et plus personne ne pourrait lancer de
 * restauration.
 *
 * ⚠️ **Dans le doute, « non ».** Un faux positif enferme l'utilisateur dans un
 * champ de mot de passe qu'aucune valeur ne satisfera. Un faux négatif, lui,
 * se termine par un refus du hub qui nomme le problème.
 */

/** Combien d'octets de tête suffisent. L'en-tête local en fait 30. */
export const SEALED_PROBE_BYTES = 30;

/** Signature d'une entrée locale de ZIP. */
const LOCAL_FILE_HEADER = [0x50, 0x4b, 0x03, 0x04];

/** Bit 0 du drapeau général : le contenu de l'entrée est chiffré. */
const FLAG_ENCRYPTED = 0x0001;

export function isSealedZip(head: Uint8Array): boolean {
  if (head.length < 8) return false;
  for (let i = 0; i < LOCAL_FILE_HEADER.length; i += 1) {
    if (head[i] !== LOCAL_FILE_HEADER[i]) return false;
  }
  const flags = head[6] | (head[7] << 8);
  return (flags & FLAG_ENCRYPTED) !== 0;
}

/**
 * Même question, posée à un fichier choisi dans l'explorateur.
 *
 * `File.slice` ne lit que la tranche demandée : le fichier entier ne passe
 * jamais par la mémoire de l'onglet.
 */
export async function fileIsSealedZip(file: Blob): Promise<boolean> {
  try {
    const head = await file.slice(0, SEALED_PROBE_BYTES).arrayBuffer();
    return isSealedZip(new Uint8Array(head));
  } catch {
    // Fichier devenu illisible entre le choix et la lecture. Ce n'est pas à
    // cet écran de le dire : le hub refusera l'envoi en le nommant.
    return false;
  }
}
