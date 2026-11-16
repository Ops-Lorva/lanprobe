import { describe, expect, it } from 'vitest';
import { hubMark, internetMark, targetMark, type Mark } from './status';

/**
 * Un état = une forme + un mot (+ une couleur), comme sur le hub
 * (`web-ui/src/lib/components/StatusMark.svelte`) : la couleur seule ne se lit
 * ni en niveaux de gris ni par un daltonien.
 */
const forme = (m: Mark) => `${m.tone}/${m.shape}`;

describe('une cible surveillée', () => {
  it('répond : disque plein, vert', () => {
    expect(forme(targetMark(true))).toBe('ok/dot');
    expect(targetMark(true).key).toBe('status.target_alive');
  });
  it('ne répond pas : carré rouge, le seul état qui appelle une action', () => {
    expect(forme(targetMark(false))).toBe('bad/square');
    expect(targetMark(false).key).toBe('status.target_down');
  });
  it('pas encore de mesure : anneau neutre', () => {
    expect(forme(targetMark(null))).toBe('none/ring');
  });
});

describe('internet', () => {
  it('limité se dit à demi, en orange', () => {
    expect(forme(internetMark('limited'))).toBe('warn/half');
  });
  it('coupé est un carré', () => {
    expect(forme(internetMark('offline'))).toBe('bad/square');
  });
  it('inconnu ne prétend rien', () => {
    expect(forme(internetMark(null))).toBe('none/ring');
  });
});

describe('le rattachement au hub', () => {
  const base = { enrolled: true, url: 'https://hub.exemple.fr', state: 'ok', buffered_points: 0, last_error: null, refused_status: null };

  it('non rattachée : neutre, rien à signaler', () => {
    expect(hubMark(null).key).toBe('hub_state.detached');
    expect(hubMark({ ...base, enrolled: false, state: 'off' }).key).toBe('hub_state.detached');
    expect(forme(hubMark({ ...base, enrolled: false, state: 'off' }))).toBe('none/ring');
  });
  it('rattachée, en attente du premier battement', () => {
    expect(hubMark({ ...base, state: 'off' }).key).toBe('hub_state.pending');
  });
  it('rattachée et battant : disque vert', () => {
    expect(forme(hubMark(base))).toBe('ok/dot');
    expect(hubMark(base).key).toBe('hub_state.attached');
  });
  // `degraded` se répare seul : orange, en anneau (on ne sait pas), avec le tampon.
  it('injoignable : anneau orange, et le nombre de points en tampon', () => {
    const m = hubMark({ ...base, state: 'degraded', buffered_points: 412 });
    expect(forme(m)).toBe('warn/ring');
    expect(m.key).toBe('hub_state.unreachable');
    expect(m.values).toMatchObject({ n: 412 });
  });
  // 🔴 Un 403 d'un proxy devant un hub sain n'est pas « injoignable ».
  it('a répondu et refusé : le code, pas « injoignable »', () => {
    const m = hubMark({ ...base, state: 'degraded', refused_status: 403 });
    expect(m.key).toBe('hub_state.refused');
    expect(m.values).toMatchObject({ code: 403 });
  });
  // `broken` ne se répare jamais seul : il faut ré-enrôler.
  it('révoquée : carré rouge', () => {
    expect(forme(hubMark({ ...base, state: 'broken' }))).toBe('bad/square');
    expect(hubMark({ ...base, state: 'broken' }).key).toBe('hub_state.revoked');
  });
});
