import { describe, expect, it } from 'vitest';
import { probeLiveness, HEARTBEAT_SECS, OFFLINE_AFTER_SECS } from './probe-liveness';

const AT = 1_788_015_096_000; // heure courante, en millisecondes
const NOW = AT / 1000;

describe('probeLiveness', () => {
  it('ne dit « en ligne » que dans la cadence de battement', () => {
    // Le défaut de production : un Mac rallumé bat une fois puis se tait. Le
    // hub tolérait trois battements manqués et affichait vert pendant tout ce
    // temps — une disponibilité qu'il ne pouvait pas connaître.
    expect(probeLiveness(NOW - 30, AT).state).toBe('online');
    expect(probeLiveness(NOW - 90, AT).state).toBe('silent');
  });

  it('bascule à la cadence exacte, pas après', () => {
    expect(probeLiveness(NOW - (HEARTBEAT_SECS - 1), AT).state).toBe('online');
    expect(probeLiveness(NOW - HEARTBEAT_SECS, AT).state).toBe('silent');
  });

  it('reste silencieuse jusqu’au seuil, hors ligne au-delà', () => {
    expect(probeLiveness(NOW - (OFFLINE_AFTER_SECS - 1), AT).state).toBe('silent');
    expect(probeLiveness(NOW - OFFLINE_AFTER_SECS, AT).state).toBe('offline');
  });

  it('rapporte le silence en secondes, pour que l’écran dise depuis quand', () => {
    expect(probeLiveness(NOW - 754, AT).silentFor).toBe(754);
  });

  it('distingue « jamais vue » de « hors ligne depuis 1970 »', () => {
    // `silentFor` nul est le signal : sans lui, l'écran calculerait un âge
    // depuis l'epoch et annoncerait un silence de cinquante-six ans.
    expect(probeLiveness(null, AT)).toEqual({ state: 'offline', silentFor: null });
    expect(probeLiveness(undefined, AT)).toEqual({ state: 'offline', silentFor: null });
  });

  it('traite un battement daté du futur comme récent', () => {
    // Sonde dont l'horloge avance sur celle du hub. « il y a -3 min » ne veut
    // rien dire, et la seule chose qu'on sache, c'est qu'elle vient de parler.
    expect(probeLiveness(NOW + 180, AT)).toEqual({ state: 'online', silentFor: 0 });
  });

  it('accepte des seuils explicites : le hub ne garde pas les siens en dur', () => {
    // Cadence de 5 s en mode temps réel, seuil d'alerte de 300 s.
    expect(probeLiveness(NOW - 6, AT, 5, 300).state).toBe('silent');
    expect(probeLiveness(NOW - 301, AT, 5, 300).state).toBe('offline');
  });
});
