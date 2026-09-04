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

/** Une valeur d'où qu'elle vienne, ramenée à une des trois langues — ou rien. */
function known(candidate: string | null | undefined): Lang | null {
  // On ne compare que le sous-tag de langue : la valeur peut arriver en
  // « fr-CA » ou « es-419 », et seul « fr » exact serait reconnu.
  const tag = candidate?.toLowerCase().split('-')[0];
  return LANGS.find((l) => l === tag) ?? null;
}

/**
 * Quelle langue afficher, entre les trois sources — dans cet ordre.
 *
 * 🔴 **La base fait foi.** Le réglage du compte (`GET /api/me` → `lang`) prime
 * sur tout : c'est ce qui fait qu'un changement de navigateur ne ramène plus
 * l'anglais. `localStorage` n'est qu'un **cache local**, gardé pour éviter un
 * éclair de langue au chargement avant que `/api/me` n'ait répondu.
 *
 * ⚠️ `null` côté hub veut dire « rien de réglé », **pas** « anglais » : un
 * défaut inventé côté serveur imposerait une langue à tous les comptes déjà en
 * service. On retombe alors sur le comportement d'avant — cache, puis
 * navigateur, puis anglais.
 */
export function langToApply(
  server: Lang | null | undefined,
  cached: Lang | null,
  browserTag: string | undefined,
): Lang {
  return known(server) ?? known(cached) ?? known(browserTag) ?? 'en';
}

/** Préférence explicite, sinon langue du navigateur, sinon anglais. */
export function preferredLang(): Lang {
  return langToApply(null, stored(), navigator.language);
}

export function initI18n() {
  const lang = preferredLang();
  init({ fallbackLocale: 'en', initialLocale: lang });
  document.documentElement.setAttribute('lang', lang);
}

/**
 * Applique ce que le compte a réglé, une fois `/api/me` revenu.
 *
 * ⚠️ Le cache local est **réaligné** au passage : sinon le prochain chargement
 * repartirait sur l'ancienne valeur, et l'éclair de langue qu'on cherche à
 * éviter se produirait à chaque fois.
 */
export function applyServerLang(server: Lang | null | undefined): Lang {
  const lang = langToApply(server, stored(), navigator.language);
  setLang(lang);
  return lang;
}

/**
 * Le sélecteur : on applique **tout de suite** en local, et l'enregistrement
 * côté compte suit.
 *
 * ⚠️ `save` est facultatif et son échec ne bloque rien : le sélecteur de
 * langue s'affiche aussi sur l'écran de connexion, où il n'y a pas encore de
 * compte à qui l'attacher. Une interface qui refuserait de changer de langue
 * parce que le hub a répondu `401` serait pire que le défaut corrigé.
 */
export function setLang(lang: Lang, save?: (l: Lang) => Promise<unknown>) {
  try {
    localStorage.setItem(KEY, lang);
  } catch {
    /* non bloquant */
  }
  document.documentElement.setAttribute('lang', lang);
  locale.set(lang);
  void save?.(lang).catch(() => {
    /* la langue est appliquée ; l'enregistrer est un bonus */
  });
}
