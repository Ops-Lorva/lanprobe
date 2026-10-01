/**
 * Repérer une sonde en retard de version.
 *
 * 🔴 **La référence est la version la plus récente du PARC**, pas celle
 * publiée sur GitHub : un hub auto-hébergé tourne souvent sans accès sortant,
 * et une couleur qui dépendrait d'un appel externe deviendrait fausse — ou
 * absente — exactement là où on en a besoin. Elle répond donc à « laquelle
 * traîne derrière les autres », ce qui est la question qu'on se pose devant un
 * parc.
 *
 * ⚠️ Conséquence assumée : si TOUT le parc est en retard, rien n'est signalé.
 * Le hub ne compare que ce qu'il voit.
 */

/** Les trois nombres d'une version, ou `null` si ce n'en est pas une. */
function parse(version: string | null | undefined): [number, number, number] | null {
  if (!version) return null;
  const m = /^(\d+)\.(\d+)\.(\d+)/.exec(version.trim());
  if (!m) return null;
  return [Number(m[1]), Number(m[2]), Number(m[3])];
}

/** Négatif si `a` est antérieure à `b`. */
function compare(a: [number, number, number], b: [number, number, number]): number {
  for (let i = 0; i < 3; i++) {
    if (a[i] !== b[i]) return a[i] - b[i];
  }
  return 0;
}

/**
 * La plus récente des versions lisibles. `null` s'il n'y en a aucune.
 *
 * ⚠️ Comparée comme des NOMBRES : « 2.10.0 » est plus récente que « 2.9.0 »,
 * alors qu'elle la précède dans l'ordre alphabétique.
 */
export function newestVersion(versions: (string | null | undefined)[]): string | null {
  let best: { raw: string; parsed: [number, number, number] } | null = null;
  for (const raw of versions) {
    const parsed = parse(raw);
    if (!parsed || !raw) continue;
    if (!best || compare(parsed, best.parsed) > 0) best = { raw: raw.trim(), parsed };
  }
  return best?.raw ?? null;
}

/**
 * Vrai si cette sonde est derrière la référence.
 *
 * 🔴 Une version **illisible n'est pas périmée**, et une sonde **en avance**
 * (build de test) non plus. Colorer sur un doute enverrait mettre à jour une
 * machine qui n'en a pas besoin — et sur un parc de client, c'est un
 * déplacement.
 */
export function isBehind(
  version: string | null | undefined,
  newest: string | null | undefined,
): boolean {
  const mine = parse(version);
  const reference = parse(newest);
  if (!mine || !reference) return false;
  return compare(mine, reference) < 0;
}
