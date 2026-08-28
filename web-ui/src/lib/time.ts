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
