import { readable } from 'svelte/store';

/**
 * Horloge partagée : « il y a 2 min » doit vieillir tout seul. Sans ce tick,
 * un parc laissé ouvert affiche indéfiniment le délai calculé au chargement —
 * exactement le mensonge que le battement de cœur sert à éviter.
 */
export const now = readable(Date.now(), (set) => {
  const id = setInterval(() => set(Date.now()), 15_000);
  return () => clearInterval(id);
});

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ['second', 60],
  ['minute', 60],
  ['hour', 24],
  ['day', 7],
  ['week', 4.345],
  ['month', 12],
  ['year', Infinity],
];

/**
 * Délai relatif lisible. `seconds` est un epoch en secondes (format du contrat).
 * On passe par Intl : les règles de pluriel et d'ordre des mots sont celles de
 * la locale, pas des chaînes traduites à la main.
 */
export function relativeTime(seconds: number | null | undefined, locale: string, at: number): string {
  if (seconds == null) return '—';
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: 'auto', style: 'short' });
  let delta = (seconds * 1000 - at) / 1000;
  for (const [unit, step] of UNITS) {
    if (Math.abs(delta) < step || step === Infinity) {
      return rtf.format(Math.round(delta), unit);
    }
    delta /= step;
  }
  return '—';
}

/** Date complète, pour le `title` d'un délai relatif : le relatif ne suffit pas en incident. */
export function absoluteTime(seconds: number | null | undefined, locale: string): string {
  if (seconds == null) return '—';
  return new Intl.DateTimeFormat(locale, { dateStyle: 'medium', timeStyle: 'medium' }).format(
    new Date(seconds * 1000),
  );
}

/**
 * Horodatage d'une ligne de journal : date courte, heure à la seconde.
 *
 * `dateStyle: 'medium'` écrirait « 29 août 2026 à 09:51:36 » — trop long pour
 * une colonne, et l'année en toutes lettres n'apprend rien dans un journal
 * qu'on parcourt. La seconde, elle, est indispensable : c'est par elle qu'on
 * recoupe une ligne avec les logs du conteneur.
 */
export function logTime(seconds: number | null | undefined, locale: string): string {
  if (seconds == null) return '—';
  return new Intl.DateTimeFormat(locale, {
    dateStyle: 'short',
    timeStyle: 'medium',
  }).format(new Date(seconds * 1000));
}

/** Date seule : pour « enrôlée le », l'heure à la seconde n'apprend rien. */
export function dateOnly(seconds: number | null | undefined, locale: string): string {
  if (seconds == null) return '—';
  return new Intl.DateTimeFormat(locale, { dateStyle: 'medium' }).format(new Date(seconds * 1000));
}

/** Heure courte pour les axes de graphiques. */
export function axisTime(ms: number, locale: string, spanMs: number): string {
  const opts: Intl.DateTimeFormatOptions =
    spanMs > 36 * 3600 * 1000
      ? { day: '2-digit', month: '2-digit' }
      : { hour: '2-digit', minute: '2-digit' };
  return new Intl.DateTimeFormat(locale, opts).format(new Date(ms));
}

/** Horodatage complet dans les infobulles de graphiques. */
export function tooltipTime(ms: number, locale: string): string {
  return new Intl.DateTimeFormat(locale, { dateStyle: 'short', timeStyle: 'medium' }).format(
    new Date(ms),
  );
}

/**
 * Taille lisible. Les symboles changent de langue — un anglophone lit « GB »,
 * pas « Go » ; l'inverse fait douter de la valeur elle-même.
 */
export function humanBytes(n: number | null | undefined, locale: string): string {
  if (n == null || !Number.isFinite(n)) return '—';
  const units = locale.startsWith('fr')
    ? ['o', 'Ko', 'Mo', 'Go', 'To']
    : ['B', 'KB', 'MB', 'GB', 'TB'];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(v)} ${units[i]}`;
}

/** Compacte les grands nombres : 1 284 → 1,3 k. Utilisé sur les compteurs de tampon. */
export function compactNumber(n: number, locale: string): string {
  return new Intl.NumberFormat(locale, { notation: 'compact', maximumFractionDigits: 1 }).format(n);
}

/**
 * Secondes restantes avant une échéance (`until`, epoch en secondes).
 *
 * ⚠️ `0` couvre les trois cas « c'est fini » — absente, nulle, dépassée — et
 * rien ne peut ressortir négatif. Le mode temps réel ne garde que cette
 * échéance : c'est la comparaison à l'heure courante qui dit s'il tourne, et
 * elle doit donner la même réponse à l'écran et dans le hub.
 */
export function secondsLeft(until: number | null | undefined, atMs: number): number {
  if (until == null) return 0;
  return Math.max(0, Math.ceil(until - atMs / 1000));
}

/**
 * Compte à rebours court : « 28 min », « 45 s ».
 *
 * ⚠️ Pas de « 27:43 » à la seconde près, et c'est délibéré : l'horloge
 * partagée ne bat que toutes les 15 s. Un chiffre de secondes figé un quart de
 * minute se lirait comme une minuterie bloquée, donc comme une panne.
 */
export function countdown(seconds: number, locale: string): string {
  const [value, unit]: [number, 'second' | 'minute'] =
    seconds < 60 ? [Math.round(seconds), 'second'] : [Math.round(seconds / 60), 'minute'];
  return new Intl.NumberFormat(locale, { style: 'unit', unit, unitDisplay: 'short' }).format(value);
}
