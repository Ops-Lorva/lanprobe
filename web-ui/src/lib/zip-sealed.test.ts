import { describe, expect, it } from 'vitest';
import { isSealedZip, SEALED_PROBE_BYTES } from './zip-sealed';

/** Un en-tête d'entrée locale de ZIP, avec le drapeau général demandé. */
function header(flags: number, signature = [0x50, 0x4b, 0x03, 0x04]): Uint8Array {
  const head = new Uint8Array(30);
  head.set(signature, 0);
  // Version nécessaire pour extraire — sans intérêt ici, mais elle occupe la place.
  head[4] = 0x14;
  head[5] = 0x00;
  head[6] = flags & 0xff;
  head[7] = (flags >> 8) & 0xff;
  return head;
}

describe('détection d’une archive protégée par mot de passe', () => {
  it('lit le bit 0 du drapeau général de la première entrée', () => {
    expect(isSealedZip(header(0x0000))).toBe(false);
    expect(isSealedZip(header(0x0001))).toBe(true);
  });

  it('ne confond pas le bit de chiffrement avec les autres drapeaux', () => {
    // Bit 3 = descripteur de données, posé par tout écrivain en flux. Le lire
    // comme « protégée » ferait réclamer un mot de passe sur une archive en
    // clair, et la restauration deviendrait impossible à lancer.
    expect(isSealedZip(header(0x0008))).toBe(false);
    // Bit 11 = nom de fichier en UTF-8. Le `zip` du hub le pose.
    expect(isSealedZip(header(0x0800))).toBe(false);
    // Les deux, plus le chiffrement.
    expect(isSealedZip(header(0x0809))).toBe(true);
  });

  it('ne dit jamais « protégée » de ce qui n’est pas un ZIP', () => {
    // ⚠️ Un faux positif ici réclamerait un mot de passe pour un fichier qui
    // n'en a pas, et personne ne pourrait passer l'écran. Dans le doute, on
    // laisse le hub refuser le fichier : lui sait le nommer.
    expect(isSealedZip(header(0x0001, [0x89, 0x50, 0x4e, 0x47]))).toBe(false);
    expect(isSealedZip(new Uint8Array(0))).toBe(false);
    expect(isSealedZip(new Uint8Array([0x50, 0x4b, 0x03]))).toBe(false);
    // Une archive vide commence par la fin du répertoire central, pas par une
    // entrée locale : rien à chiffrer, donc rien à réclamer.
    expect(isSealedZip(header(0x0001, [0x50, 0x4b, 0x05, 0x06]))).toBe(false);
  });

  it('se contente des premiers octets du fichier', () => {
    // Le navigateur ne lit qu'une tranche : une archive de 600 Mo ne doit pas
    // passer par la mémoire de l'onglet pour qu'on sache si elle est scellée.
    expect(SEALED_PROBE_BYTES).toBeLessThanOrEqual(64);
    expect(SEALED_PROBE_BYTES).toBeGreaterThanOrEqual(8);
  });
});
