/**
 * Le détail d'une ligne d'audit, lisible par un humain.
 *
 * Le hub écrit ses détails pour lui-même : identifiants, horodatages Unix,
 * octets bruts (`site 0b7c… · 1794…..1794… · 3 sonde(s)`). C'est juste et ça
 * reste la vérité en base ; à l'écran, on lit des noms, des dates et des Mo.
 *
 * ⚠️ Rien n'est inventé. Un identifiant dont on ne connaît pas le nom fait
 * tomber son segment (« site … ») ; seul dans la phrase, il est raccourci à
 * huit caractères plutôt que retiré, pour que la phrase garde son sens.
 */

const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi;
/** Un horodatage Unix plausible (2001 → 2286) : dix chiffres commençant par 1. */
const STAMP = /\b1\d{9}\b/g;
const RANGE = /\b(1\d{9})\.\.(1\d{9})\b/g;
const SEP = ' · ';

type T = (key: string, values?: Record<string, unknown>) => string;

export interface DetailOptions {
  lang: string;
  /** Noms connus, par identifiant : sites, sondes, appareils. */
  names: Record<string, string>;
  /** Instant de lecture, en ms : l'année courante ne s'écrit pas. */
  at: number;
  t: T;
}

/** Une taille toujours en Mo — le journal compare des archives entre elles. */
export function megabytes(bytes: number, lang: string): string {
  // Même base que `humanBytes` : 1 Mo = 1024 × 1024 octets.
  const mb = bytes / 1_048_576;
  const unit = lang.startsWith('fr') ? 'Mo' : 'MB';
  const fmt = new Intl.NumberFormat(lang, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  if (mb < 0.05) return `< ${fmt.format(0.1)} ${unit}`;
  return `${fmt.format(mb)} ${unit}`;
}

const pad = (n: number) => String(n).padStart(2, '0');

function day(seconds: number, withYear: boolean): string {
  const d = new Date(seconds * 1000);
  const base = `${pad(d.getDate())}/${pad(d.getMonth() + 1)}`;
  return withYear ? `${base}/${d.getFullYear()}` : base;
}

export function auditDetail(raw: string | null | undefined, o: DetailOptions): string {
  if (!raw) return '';
  const year = new Date(o.at).getFullYear();
  let s = raw;

  s = s.replace(/(\d+) octets\b/g, (_, n) => megabytes(Number(n), o.lang));

  s = s.replace(RANGE, (_, a, b) => {
    const ya = new Date(Number(a) * 1000).getFullYear();
    const yb = new Date(Number(b) * 1000).getFullYear();
    const full = ya !== year || yb !== year;
    return `${day(Number(a), full)} → ${day(Number(b), full)}`;
  });
  s = s.replace(STAMP, (m) => {
    const d = new Date(Number(m) * 1000);
    const full = d.getFullYear() !== year;
    return `${day(Number(m), full)} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
  });

  // Une période relative est toujours un segment entier : `-7d`, `-24h`.
  const segments = s.split(SEP).map((seg) => {
    const rel = /^-(\d+)([dh])$/.exec(seg.trim());
    if (!rel) return seg;
    return o.t(rel[2] === 'd' ? 'audit.period_days' : 'audit.period_hours', {
      values: { n: Number(rel[1]) },
    });
  });

  const alone = segments.length === 1;
  const named = segments
    .map((seg) => {
      let unknown = false;
      const out = seg.replace(UUID, (id) => {
        const name = o.names[id] ?? o.names[id.toLowerCase()];
        if (name) return name;
        unknown = true;
        return id.slice(0, 8);
      });
      if (unknown && !alone) return null;
      // « site Atelier Brunet » : le nom suffit.
      return out.replace(/^site /, '');
    })
    .filter((seg): seg is string => seg !== null && seg.trim() !== '');

  // « 3 sonde(s) » → « 3 sondes » : le nombre qui précède décide.
  return named
    .map((seg) =>
      seg.replace(/(\d+)([^\d]*)/g, (_, n, rest: string) =>
        n + rest.replace(/\(s\)/g, Number(n) > 1 ? 's' : ''),
      ),
    )
    .join(', ');
}

const ARCHIVE = /^lanprobe-backup-(\d{4})(\d{2})(\d{2})/;

/**
 * La cible d'une ligne, en mots.
 *
 * 🔴 Le nom vient du hub (`target_name`, résolu à la lecture). Sans nom, un
 * identifiant est raccourci et marqué `mono` : l'écran le garde entier en
 * infobulle, c'est lui qui permet de recouper deux lignes.
 */
export function auditTarget(
  e: { target: string | null; target_name?: string | null },
  o: DetailOptions,
): { text: string; mono: boolean } {
  if (e.target_name) return { text: e.target_name, mono: false };
  if (!e.target) return { text: '', mono: false };
  const arch = ARCHIVE.exec(e.target);
  if (arch) {
    const [, y, m, d] = arch;
    const date = Number(y) === new Date(o.at).getFullYear() ? `${d}/${m}` : `${d}/${m}/${y}`;
    return { text: o.t('audit.archive_of', { values: { date } }), mono: false };
  }
  if (new RegExp(`^${UUID.source}$`, 'i').test(e.target)) {
    return { text: e.target.slice(0, 8), mono: true };
  }
  return { text: e.target, mono: false };
}
