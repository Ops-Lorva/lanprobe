/**
 * Les lectures de la fiche suivent la sonde du tour qui les a lancées.
 *
 * Constaté en prod : `GET /api/probes/undefined/uptime` et
 * `/api/probes/undefined/public-ips`. Ces deux lectures partent APRÈS les
 * courbes, qui prennent plusieurs secondes ; quitter la fiche entre-temps
 * démonte la vue, et `id` — qui suit `$route.id` — ne vaut plus rien quand
 * elles partent enfin.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { probeCycle } from './probe-cycle';

describe('probeCycle', () => {
  it('garde l’identifiant lu au départ du tour', () => {
    let route: string | undefined = 'sonde-a';
    const cycle = probeCycle(() => route);
    route = undefined;
    expect(cycle.id).toBe('sonde-a');
  });

  it('se dit périmé quand la fiche a été quittée', () => {
    let route: string | undefined = 'sonde-a';
    const cycle = probeCycle(() => route);
    expect(cycle.stale()).toBe(false);
    route = undefined;
    expect(cycle.stale()).toBe(true);
  });

  it('se dit périmé quand on est passé à une autre sonde', () => {
    let route: string | undefined = 'sonde-a';
    const cycle = probeCycle(() => route);
    route = 'sonde-b';
    expect(cycle.stale()).toBe(true);
  });

  it('naît périmé sans identifiant : rien ne doit partir', () => {
    expect(probeCycle(() => undefined).stale()).toBe(true);
  });
});

describe('la fiche d’une sonde', () => {
  const vue = readFileSync(new URL('../views/ProbeView.svelte', import.meta.url), 'utf8');

  it('passe l’identifiant du tour aux lectures qui suivent les courbes', () => {
    expect(vue).toContain('probeCycle(() => id)');
    expect(vue).toContain('if (cycle.stale()) return;');
    expect(vue).toContain('loadPublicIps(choice, cycle.id)');
    expect(vue).toContain('loadUptime(choice, cycle.id)');
  });
});
