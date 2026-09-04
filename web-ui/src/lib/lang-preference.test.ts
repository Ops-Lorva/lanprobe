import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from './api';
import { langToApply } from './i18n';

/**
 * La langue, côté interface — et les DEUX réglages qu'elle recouvre.
 *
 * 🔴 Le défaut corrigé : le sélecteur n'écrivait que dans `localStorage`.
 * Changer de navigateur ramenait l'anglais, deux personnes sur le même hub le
 * voyaient chacune dans sa langue, et — le point qui compte — **le même client
 * pouvait recevoir deux rapports dans deux langues** selon qui avait cliqué.
 *
 * Ce sont deux questions, et les confondre laissait le défaut entier :
 *
 * - la langue de l'**interface** suit le COMPTE (`GET /api/me` → `lang`) ;
 * - la langue du **rapport** suit le SITE (`PATCH /api/sites/{id}` →
 *   `report_locale`), parce que le classeur part chez un client.
 */

describe('langToApply', () => {
  it('fait primer ce que dit le hub sur le cache local', () => {
    // 🔴 La base fait foi. `localStorage` n'est qu'un cache pour éviter un
    // éclair au chargement — s'il gagnait, on n'aurait rien corrigé.
    expect(langToApply('es', 'fr', 'fr-FR')).toBe('es');
  });

  it('retombe sur le cache local quand le compte n’a rien réglé', () => {
    // ⚠️ `null` du hub veut dire « rien de réglé », pas « anglais ». Un défaut
    // inventé côté hub imposerait une langue à tous les comptes existants.
    expect(langToApply(null, 'fr', 'en-US')).toBe('fr');
  });

  it('retombe sur le navigateur quand rien n’est réglé nulle part', () => {
    // C'est le comportement d'aujourd'hui, et il ne doit pas changer pour les
    // hubs déjà en service.
    expect(langToApply(null, null, 'fr-CA')).toBe('fr');
    expect(langToApply(undefined, null, 'es-419')).toBe('es');
  });

  it('finit en anglais plutôt que de rendre une langue que le produit ne parle pas', () => {
    expect(langToApply(null, null, 'de-DE')).toBe('en');
    expect(langToApply(null, null, undefined)).toBe('en');
  });

  it('ignore une valeur que le produit ne parle pas, d’où qu’elle vienne', () => {
    // Une langue retirée du produit, ou une valeur écrite à la main en base,
    // ne doit pas laisser l'interface sans libellés.
    expect(langToApply('de' as never, 'fr', 'en')).toBe('fr');
    expect(langToApply(null, 'de' as never, 'es')).toBe('es');
  });
});

/** Dernier appel `fetch` observé. */
function captureFetch(body: unknown, status = 200) {
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

describe('la langue de l’interface', () => {
  it('s’enregistre sur le compte, pas seulement dans le navigateur', async () => {
    const spy = captureFetch({ ok: true, lang: 'es' });
    await api.setUiLang('es');
    const [url, init] = spy.mock.calls[0];
    if (!init) throw new Error('aucune option de requête');
    expect(url).toBe('/api/me/lang');
    expect(init.method).toBe('POST');
    expect(JSON.parse(String(init.body))).toEqual({ lang: 'es' });
  });

  it('se retire avec `null` — c’est un réglage, pas un aller simple', async () => {
    const spy = captureFetch({ ok: true, lang: null });
    await api.setUiLang(null);
    const [, init] = spy.mock.calls[0];
    expect(JSON.parse(String(init!.body))).toEqual({ lang: null });
  });
});

describe('la langue des rapports d’un site', () => {
  it('part avec le nom, là où on règle le site', async () => {
    const spy = captureFetch({ site_id: 's1', name: 'Durand', report_locale: 'es' });
    await api.renameSite('s1', 'Durand', 'es');
    const [url, init] = spy.mock.calls[0];
    if (!init) throw new Error('aucune option de requête');
    expect(url).toBe('/api/sites/s1');
    expect(init.method).toBe('PATCH');
    expect(JSON.parse(String(init.body))).toEqual({ name: 'Durand', report_locale: 'es' });
  });

  it('n’est PAS envoyée par un renommage seul', async () => {
    // 🔴 Trois états, pas deux : champ absent = « on n'y touche pas ». Envoyer
    // `null` par défaut effacerait la langue d'un client à chaque renommage,
    // en silence, et le suivant s'en apercevrait sur le document remis.
    const spy = captureFetch({ site_id: 's1', name: 'Durand SA' });
    await api.renameSite('s1', 'Durand SA');
    const [, init] = spy.mock.calls[0];
    expect(JSON.parse(String(init!.body))).toEqual({ name: 'Durand SA' });
  });

  it('se retire avec un `null` explicite', async () => {
    const spy = captureFetch({ site_id: 's1', name: 'Durand', report_locale: null });
    await api.renameSite('s1', 'Durand', null);
    const [, init] = spy.mock.calls[0];
    expect(JSON.parse(String(init!.body))).toEqual({ name: 'Durand', report_locale: null });
  });
});
