import { describe, expect, it } from 'vitest';
import { auditActor } from './audit-actors';

describe('auditActor', () => {
  it('rend le compte quand il y en a un', () => {
    expect(auditActor({ actor: 'claire', actor_kind: null })).toEqual({
      kind: 'account',
      name: 'claire',
    });
  });

  it('distingue un geste DU HUB d’une tentative anonyme', () => {
    // 🔴 Les deux s'écrivent sans acteur, et l'écran affichait « anonyme » pour
    // les deux. Or « anonyme » dit une chose précise : une tentative de
    // connexion sur un compte inconnu, un humain qu'on n'a pas pu identifier.
    // Une sauvegarde planifiée, elle, est faite par le hub — chercher qui s'est
    // connecté pour un geste que personne n'a fait est une perte de temps, et
    // au pire une fausse alerte de sécurité.
    expect(auditActor({ actor: null, actor_kind: 'system' })).toEqual({ kind: 'system' });
    expect(auditActor({ actor: null, actor_kind: null })).toEqual({ kind: 'unknown' });
  });

  it('ne prend JAMAIS une marque qu’elle ne connaît pas pour « système »', () => {
    // ⚠️ Même règle qu'à l'origine d'une commande (§ 26) : un hub plus récent,
    // un onglet resté ouvert pendant une mise à jour. Une valeur inconnue se
    // lit « inconnue », jamais brute — et surtout pas « système », qui
    // affirmerait que le hub a fait quelque chose dont on ne sait rien.
    expect(auditActor({ actor: null, actor_kind: 'cron-de-l-hote' })).toEqual({ kind: 'unknown' });
  });

  it('ignore la marque quand un compte est nommé', () => {
    // Le compte est le fait le plus fort : une marque qui le contredirait ne
    // doit pas effacer le nom de qui a agi.
    expect(auditActor({ actor: 'claire', actor_kind: 'system' })).toEqual({
      kind: 'account',
      name: 'claire',
    });
  });

  it('traite un acteur vide comme une absence', () => {
    // Une chaîne vide en base n'est pas un compte : l'afficher donnerait une
    // cellule blanche qu'on ne saurait pas lire.
    expect(auditActor({ actor: '   ', actor_kind: null })).toEqual({ kind: 'unknown' });
  });
});
