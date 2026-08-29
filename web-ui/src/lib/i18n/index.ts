import { register, init, locale } from 'svelte-i18n';

/**
 * Même mécanisme que l'app desktop (`src/lib/i18n/index.ts`) : svelte-i18n,
 * catalogues JSON chargés à la demande. Les trois langues sont celles de
 * l'app : une sonde et le hub qui la pilote doivent parler la même.
 */
export type Lang = 'fr' | 'en' | 'es';

export const LANGS: Lang[] = ['fr', 'en', 'es'];

register('fr', () => import('./fr.json'));
register('en', () => import('./en.json'));
register('es', () => import('./es.json'));

const KEY = 'lanprobe.hub.lang';

function stored(): Lang | null {
  try {
    const v = localStorage.getItem(KEY);
    return LANGS.includes(v as Lang) ? (v as Lang) : null;
  } catch {
    return null;
  }
}

/** Préférence explicite, sinon langue du navigateur, sinon anglais. */
export function preferredLang(): Lang {
  const explicit = stored();
  if (explicit) return explicit;
  // `navigator.language` vaut « fr-CA », « es-419 »… : on ne compare que le
  // sous-tag de langue, sinon seul « fr » exact serait reconnu.
  const tag = navigator.language?.toLowerCase().split('-')[0];
  return LANGS.find((l) => l === tag) ?? 'en';
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
