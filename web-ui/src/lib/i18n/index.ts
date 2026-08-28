import { register, init, locale } from 'svelte-i18n';

/**
 * Même mécanisme que l'app desktop (`src/lib/i18n/index.ts`) : svelte-i18n,
 * catalogues JSON chargés à la demande. Le hub n'expose que FR et EN.
 */
export type Lang = 'fr' | 'en';

export const LANGS: Lang[] = ['fr', 'en'];

register('fr', () => import('./fr.json'));
register('en', () => import('./en.json'));

const KEY = 'lanprobe.hub.lang';

function stored(): Lang | null {
  try {
    const v = localStorage.getItem(KEY);
    return v === 'fr' || v === 'en' ? v : null;
  } catch {
    return null;
  }
}

/** Préférence explicite, sinon langue du navigateur, sinon anglais. */
export function preferredLang(): Lang {
  return stored() ?? (navigator.language?.toLowerCase().startsWith('fr') ? 'fr' : 'en');
}

export function initI18n() {
  const lang = preferredLang();
  init({ fallbackLocale: 'en', initialLocale: lang });
  document.documentElement.setAttribute('lang', lang);
}

export function setLang(lang: Lang) {
  try {
    localStorage.setItem(KEY, lang);
  } catch {
    /* non bloquant */
  }
  document.documentElement.setAttribute('lang', lang);
  locale.set(lang);
}
