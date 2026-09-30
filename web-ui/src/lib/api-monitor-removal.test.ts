/**
 * Le retrait d'une surveillance depuis le hub, quand la sonde est éteinte.
 *
 * ⚠️ Test écrit par la QA après un exercice bout-en-bout sur le hub de
 * production (02/09/2026, hub 1.4.0, sonde headless 2.3.0). Il échoue
 * aujourd'hui, et c'est son objet : le contrat §20 dit
 *
 *   « `POST /api/probes/{id}/monitors/remove` — écrit la suppression
 *     IMMÉDIATEMENT, sans attendre la sonde : sinon retirer une cible pendant
 *     qu'elle est hors ligne perdait la décision jusqu'à son retour. »
 *
 * La route existe côté hub et fonctionne — vérifié en production : sonde
 * arrêtée, `removed_at` écrit dans la seconde. Mais **le client HTTP de
 * l'interface ne l'appelle nulle part** : le bouton « Retirer » de
 * `ProbeView.svelte` empile une commande `remove_monitor`, qui n'existe que
 * dans la file et n'est exécutée qu'au retour de la sonde. Le paquet servi en
 * production ne contient d'ailleurs qu'une seule route `monitors/` :
 *
 *   $ grep -o "monitors/[a-z]*" index-Bmz-Nk-I.js | sort | uniq -c
 *         1 monitors/revive
 *
 * Constaté en production : sonde arrêtée à 17:45:23,
 *   — retrait de 1.1.1.1 par la ROUTE     → removed_at = 1788371126 (immédiat)
 *   — retrait de 8.8.8.8 par le BOUTON    → commande 37 « pending » pendant
 *     3 min 16 s, removed_at = 1788371323 seulement au redémarrage de la sonde.
 *
 * Le défaut que le mécanisme corrige est donc encore atteignable depuis
 * l'écran. Ce test doit passer quand `api.removeMonitor` existe et appelle la
 * route ; il ne demande RIEN d'autre — pas de retrait du bouton commande, qui
 * garde son sens quand la sonde est là.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from './api';

/** Le client est typé sans cette méthode : c'est précisément ce qui manque. */
const client = api as unknown as Record<string, unknown>;

function captureFetch() {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init: RequestInit = {}) => {
      calls.push({ url, init });
      return new Response(JSON.stringify({ ok: true, applied: true }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      });
    }),
  );
  return calls;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('retrait d’une surveillance depuis le hub', () => {
  it('expose une méthode de retrait, comme il en expose une de réactivation', () => {
    // `reviveMonitor` existe et appelle `/monitors/revive`. Son symétrique
    // manque, et c'est lui qui porte la propriété « une suppression est un
    // fait explicite, écrit tout de suite ».
    expect(typeof client.reviveMonitor).toBe('function');
    expect(typeof client.removeMonitor).toBe('function');
  });

  it('écrit le retrait par la route du hub, pas par la file de commandes', async () => {
    const calls = captureFetch();
    const removeMonitor = client.removeMonitor as
      | ((id: string, target: string) => Promise<unknown>)
      | undefined;
    if (typeof removeMonitor !== 'function') {
      throw new Error(
        'api.removeMonitor est absent : le retrait depuis le hub passe encore ' +
          'par une commande, donc il est perdu tant que la sonde est éteinte.',
      );
    }

    await removeMonitor('0c1e-probe', '8.8.8.8');

    expect(calls).toHaveLength(1);
    expect(calls[0].url).toBe('/api/probes/0c1e-probe/monitors/remove');
    expect(calls[0].init.method).toBe('POST');
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ target: '8.8.8.8' });
  });

  it('encode l’identifiant de sonde, comme la réactivation', async () => {
    const calls = captureFetch();
    const removeMonitor = client.removeMonitor as
      | ((id: string, target: string) => Promise<unknown>)
      | undefined;
    if (typeof removeMonitor !== 'function') return; // couvert par le test ci-dessus

    await removeMonitor('site/1', '10.0.30.1');
    expect(calls[0].url).toBe('/api/probes/site%2F1/monitors/remove');
  });
});
