import { describe, expect, it } from 'vitest';
import { initialScope, scopePatch, scopeSummary } from './account-scope';

const base = { role: 'viewer' as const, sites: [] as string[], all_sites: false };

describe('portée d’un compte (contrat § 17, audit RSSI F1)', () => {
  it('« tous les sites » se lit dans `all_sites`, jamais dans une liste vide', () => {
    expect(scopeSummary({ ...base, all_sites: true })).toEqual({ key: 'accounts.scope_all' });
    expect(scopeSummary({ ...base, all_sites: false, sites: [] })).toEqual({
      key: 'accounts.scope_nothing',
    });
    expect(scopeSummary({ ...base, sites: ['a', 'b'] })).toEqual({
      key: 'accounts.scope_n',
      values: { n: 2 },
    });
  });

  it('un administrateur voit toujours tout', () => {
    expect(scopeSummary({ ...base, role: 'admin' })).toEqual({ key: 'accounts.scope_admin' });
  });

  it('le dialogue part de l’état réel du compte', () => {
    expect(initialScope({ ...base, all_sites: true, sites: ['a'] })).toEqual({
      mode: 'all',
      picked: ['a'],
    });
    expect(initialScope({ ...base, sites: ['a'] })).toEqual({ mode: 'sites', picked: ['a'] });
  });

  it('envoie un ordre explicite, jamais une liste vide qui voudrait dire « tout »', () => {
    expect(scopePatch('all', ['a'])).toEqual({ all_sites: true });
    expect(scopePatch('sites', ['a'])).toEqual({ all_sites: false, sites: ['a'] });
    expect(scopePatch('sites', [])).toEqual({ all_sites: false, sites: [] });
  });

  it('un hub d’avant la v31 (sans `all_sites`) garde son ancienne lecture', () => {
    const ancien = { role: 'viewer' as const, sites: [] as string[] };
    expect(scopeSummary(ancien)).toEqual({ key: 'accounts.scope_all' });
  });
});
