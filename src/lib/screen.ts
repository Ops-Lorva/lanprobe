import { getContext, setContext } from 'svelte';

/**
 * Où un écran est rendu, selon la disposition choisie :
 *   - `page`    : seul dans la fenêtre (barre latérale) — titre h1 ;
 *   - `section` : une section parmi d'autres (trois onglets) — titre h2 ;
 *   - `cell`    : une case de la grille 3×2 (une page) — en-tête de case,
 *                 corps qui défile ;
 *   - `bare`    : dans un dialogue qui porte déjà le titre — pas d'en-tête.
 */
export type ScreenKind = 'page' | 'section' | 'cell' | 'bare';

const KEY = Symbol('lp-screen');

export function setScreenKind(kind: ScreenKind) {
  setContext(KEY, kind);
}

export function screenKind(): ScreenKind {
  return getContext<ScreenKind | undefined>(KEY) ?? 'page';
}

const NAV = Symbol('lp-nav');

/** Aller à un autre écran — fourni par la disposition « barre latérale » seulement. */
export function setNavigate(fn: (page: string) => void) {
  setContext(NAV, fn);
}

export function navigateFn(): ((page: string) => void) | undefined {
  return getContext<((page: string) => void) | undefined>(NAV);
}
