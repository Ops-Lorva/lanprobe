/** Mises en forme partagées entre la vue de parc et la fiche d'une sonde. */

/**
 * Nom de plateforme présentable.
 *
 * La sonde remonte un identifiant en minuscules (`windows`, `macos`) — c'est
 * la bonne forme pour une clé, pas pour un écran. La casse est corrigée au
 * même endroit pour les deux vues : dupliquée, elle finissait par diverger
 * (« Windows » dans le parc, « windows » sur la fiche).
 */
export function platformLabel(raw: string | null | undefined, fallback: string): string {
  if (!raw) return fallback;
  const known: Record<string, string> = {
    linux: 'Linux',
    windows: 'Windows',
    macos: 'macOS',
    freebsd: 'FreeBSD',
    openbsd: 'OpenBSD',
  };
  return known[raw.toLowerCase()] ?? raw.charAt(0).toUpperCase() + raw.slice(1);
}
