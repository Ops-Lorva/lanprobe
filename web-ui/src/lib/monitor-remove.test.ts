import { afterEach, describe, expect, it, vi } from 'vitest';
import { api, ApiError } from './api';

/**
 * Le retrait d'une surveillance **depuis le hub**.
 *
 * ⚠️ Ce n'est pas la même chose que le bouton qui empilait une commande. Une
 * commande n'agit qu'au retour de la sonde : retirer une cible pendant qu'elle
 * est éteinte, c'est justement le cas que le mécanisme d'arbitrage du contrat
 * §20 sait traiter — « le plus récent gagne », et une sonde éteinte porte par
 * construction une liste antérieure. La route l'écrit tout de suite ; la
 * commande la perdait jusqu'au redémarrage.
 *
 * D'où des tests sur la REQUÊTE émise, pas sur la réponse : le défaut qu'on
 * corrige n'était pas un mauvais calcul, c'était un appel qui n'existait pas.
 */

/** Dernier appel `fetch` observé. */
function captureFetch(body: unknown, status = 200) {
  // Les paramètres sont DÉCLARÉS : sans eux `mock.calls[0]` est typé `[]` et
  // toute lecture de l'URL ou du corps devient un transtypage aveugle.
  const spy = vi.fn(
    async (_url: string, _init?: RequestInit) =>
      new Response(JSON.stringify(body), {
        status,
        headers: { 'Content-Type': 'application/json' },
      }),
  );
  vi.stubGlobal('fetch', spy);
  return spy;
}

afterEach(() => vi.unstubAllGlobals());

describe('api.removeMonitor', () => {
  it('appelle la route du hub, et non la file de commandes', async () => {
    const spy = captureFetch({ ok: true, applied: true });
    await api.removeMonitor('0c1e-42', '8.8.8.8');

    expect(spy).toHaveBeenCalledTimes(1);
    const [url, init] = spy.mock.calls[0];
    if (!init) throw new Error('aucune option de requête');
    expect(url).toBe('/api/probes/0c1e-42/monitors/remove');
    expect(init.method).toBe('POST');
    // ⚠️ `/commands` serait le chemin perdant : la commande dort jusqu'au
    // prochain battement, et disparaît si la sonde ne revient jamais.
    expect(url).not.toContain('/commands');
  });

  it('porte la cible dans le CORPS, sous la clé que le hub lit', async () => {
    // Le hub désérialise `MonitorTarget { target }` et répond 400 « cible
    // requise » sur toute autre clé. Une faute de nom ici ne lève rien à la
    // compilation et ne se voit qu'en production.
    const spy = captureFetch({ ok: true, applied: true });
    await api.removeMonitor('sonde', '10.0.30.12');
    const [, init] = spy.mock.calls[0];
    expect(JSON.parse(String(init?.body))).toEqual({ target: '10.0.30.12' });
  });

  it('est strictement symétrique de reviveMonitor', async () => {
    // Les deux routes sont les deux sens du même fait daté. Une divergence de
    // méthode ou de corps entre elles serait un piège pour le prochain lecteur.
    const spy = captureFetch({ ok: true, applied: true });
    await api.removeMonitor('s', '1.1.1.1');
    await api.reviveMonitor('s', '1.1.1.1');
    const [removeUrl, removeInit] = spy.mock.calls[0];
    const [reviveUrl, reviveInit] = spy.mock.calls[1];

    expect(removeInit?.method).toBe(reviveInit?.method);
    expect(removeInit?.body).toBe(reviveInit?.body);
    expect(removeUrl.replace('/remove', '')).toBe(reviveUrl.replace('/revive', ''));
  });

  it('encode l’identifiant de sonde dans le chemin', async () => {
    const spy = captureFetch({ ok: true, applied: true });
    await api.removeMonitor('a/b c', '1.1.1.1');
    const [url] = spy.mock.calls[0];
    expect(url).toBe('/api/probes/a%2Fb%20c/monitors/remove');
  });

  it('remonte `applied: false` au lieu de le taire', async () => {
    // ⚠️ Le hub arbitre au plus récent : un ajout postérieur peut avoir déjà
    // tranché. Annoncer « retiré » dans ce cas serait affirmer un fait que le
    // hub vient de refuser.
    captureFetch({ ok: true, applied: false });
    await expect(api.removeMonitor('s', '1.1.1.1')).resolves.toMatchObject({ applied: false });
  });

  it('transforme un refus du hub en ApiError lisible', async () => {
    // Un rejet silencieux laisserait l'écran afficher « retirée » sur une
    // cible toujours surveillée.
    captureFetch({ error: 'rôle insuffisant pour cette action' }, 403);
    await expect(api.removeMonitor('s', '1.1.1.1')).rejects.toBeInstanceOf(ApiError);
  });
});
