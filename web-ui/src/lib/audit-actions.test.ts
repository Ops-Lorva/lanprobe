import { describe, expect, it } from 'vitest';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { AUDIT_ACTIONS } from './api';
import fr from './i18n/fr.json';
import en from './i18n/en.json';
import es from './i18n/es.json';

// Actions réellement écrites par le hub : le littéral qui suit un appel
// `audit(` / `audited(` dans les sources Rust. Sans ce lien, une action ajoutée
// côté hub s'affichait brute (« report.request ») dans le journal.
const SRC = join(__dirname, '../../../crates/lanprobe-web/src');
function emittedActions(): string[] {
  const out = new Set<string>();
  for (const file of readdirSync(SRC).filter((f) => f.endsWith('.rs'))) {
    const text = readFileSync(join(SRC, file), 'utf8')
      // Les tests ne journalisent rien : on s'arrête à leur module.
      .split('#[cfg(test)]')[0];
    for (const m of text.matchAll(/audit\w*\s*\(/g)) {
      const chunk = text.slice(m.index! + m[0].length, m.index! + m[0].length + 400);
      for (const lit of chunk.matchAll(/"([a-z_]+\.[a-z_]+(?:\.[a-z_]+)?)"/g)) {
        out.add(lit[1]);
        break;
      }
    }
    // `if demo { "user.demo" } else { "user.undemo" }` : la seconde branche
    // n'est pas le premier littéral.
    for (const lit of text.matchAll(/else \{ "([a-z_]+\.[a-z_]+)" \}/g)) out.add(lit[1]);
  }
  return [...out].sort();
}

type Catalog = { audit: { action: Record<string, string> } };

describe('actions du journal', () => {
  const emitted = emittedActions();

  it('repère bien les actions écrites par le hub', () => {
    expect(emitted).toContain('auth.login');
    expect(emitted).toContain('report.request');
  });

  it.each([
    ['fr', fr],
    ['en', en],
    ['es', es],
  ])('chaque action écrite par le hub est traduite (%s)', (_lang, catalog) => {
    const actions = (catalog as Catalog).audit.action;
    const missing = emitted.filter((a) => !actions[a.replace(/\./g, '_')]);
    expect(missing).toEqual([]);
  });

  it('chaque action écrite par le hub se filtre', () => {
    const missing = emitted.filter((a) => !AUDIT_ACTIONS.includes(a));
    expect(missing).toEqual([]);
  });
});
