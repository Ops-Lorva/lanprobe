/**
 * Rapport SLA à remettre à un client, au format Excel.
 *
 * ⚠️ Pourquoi ici et pas côté hub : produire un XLSX en Rust demanderait une
 * dépendance de plus dans un binaire qu'on veut petit, et le rapport est une
 * mise en forme, pas une donnée. Le hub rend les relevés bruts ; la mise en
 * page vit là où on la regarde.
 *
 * Ce rapport est **destiné à être lu par quelqu'un qui n'était pas là**. D'où
 * trois partis pris :
 *
 * 1. **La fenêtre est écrite en toutes lettres** sur chaque feuille. « 99,2 % »
 *    ne veut rien dire sans savoir sur quoi.
 * 2. **Les coupures sont listées une par une**, avec leur début, leur fin et
 *    leur durée. Un pourcentage seul se conteste ; une liste d'incidents datés
 *    se vérifie.
 * 3. **Une coupure en cours est marquée comme telle**, jamais close sur
 *    l'instant de l'export : écrire une heure de fin qui n'a pas eu lieu
 *    fabriquerait une durée fausse.
 */

export interface Sample {
  timestamp: number;
  /**
   * `null` = **indéterminé** : la sonde n'a pas écrit de verdict.
   *
   * ⚠️ Ni disponible, ni en panne. Le hub le déduisait de la présence d'une
   * latence — un silence transformé en fait.
   */
  alive: boolean | null;
  latency_ms?: number | null;
  /** Présent pour l'accès internet : `online` / `limited` / `offline`. */
  state?: string;
}

/**
 * Ce qui a réellement été mesuré sur la fenêtre, **calculé par le hub**.
 *
 * ⚠️ Transporté, jamais recalculé ici : le CSV du hub et le classeur doivent
 * annoncer la MÊME complétude. Deux calculs finiraient par diverger d'un point,
 * et c'est celui du document remis au client qu'on ne saurait plus expliquer.
 * Le calcul vit dans `lanprobe-core/src/sla.rs` (`compute_coverage`).
 */
export interface Coverage {
  window_secs: number;
  covered_secs: number;
  gap_secs: number;
}

export interface TargetSeries {
  ip: string;
  samples: Sample[];
  /** Absente d'un hub antérieur à la fonctionnalité — voir `coverageLabel`. */
  coverage?: Coverage;
}

/**
 * Mention de complétude à afficher, ou `null` quand il n'y a rien à dire.
 *
 * ⚠️ **`null` quand c'est complet.** Les trous d'aujourd'hui viennent surtout
 * d'une période antérieure à la fonctionnalité et doivent tendre vers zéro : un
 * « 0 % indéterminé » permanent est du bruit qui finit par masquer le cas où ça
 * compte vraiment.
 *
 * ⚠️ `null` aussi quand le hub n'envoie rien : absence d'information, on
 * n'invente pas « complet ».
 *
 * ⚠️ Une fenêtre VIDE n'est pas complète — sans cette garde, `gap_secs === 0`
 * la déclarait « tout va bien » sur une période qui n'existe pas. Le même
 * défaut a été corrigé côté Rust, trouvé par son test.
 *
 * ⚠️ Le rendu est une CHAÎNE et le restera : dans une cellule de tableur, un
 * nombre se retrouve dans une moyenne de colonne et fait naître un chiffre que
 * personne n'a calculé.
 */
export function coverageLabel(
  coverage: Coverage | undefined,
  t: Translate = (k) => k,
): string | null {
  if (!coverage) return null;
  if (coverage.window_secs <= 0) return t('sla.coverage_unknown');
  if (coverage.gap_secs === 0) return null;
  const pct = (coverage.covered_secs / coverage.window_secs) * 100;
  return t('sla.coverage_partial', { pct: pct.toFixed(1) });
}

export interface SpeedtestRow {
  started_at: number;
  engine: string | null;
  server_name: string | null;
  download_mbps: number | null;
  upload_mbps: number | null;
  latency_ms: number | null;
  jitter_ms: number | null;
  result_url: string | null;
}

export interface ScanHost {
  ip: string;
  hostname?: string | null;
  mac?: string | null;
  vendor?: string | null;
  latency_ms?: number | null;
}

export interface ScanPort {
  ip: string;
  port: number;
  proto: string;
  service?: string | null;
}

export interface Scan {
  started_at: number;
  cidr: string | null;
  hosts: ScanHost[];
  ports: ScanPort[];
}

export interface SlaPayload {
  probe: string;
  site: string;
  range: string;
  start?: number | null;
  stop?: number | null;
  generated_at: number;
  targets: TargetSeries[];
  internet: Sample[];
  speedtests: SpeedtestRow[];
  discovery: Scan | null;
  ports: Scan | null;
  /**
   * Intervalles d'adresse publique couvrant la fenêtre. Ils voyagent AVEC le
   * rapport : le tableau de l'interface et l'onglet Excel découpent les mêmes
   * relevés avec les mêmes intervalles.
   */
  public_ip_history: PublicIpInterval[];
}

export interface Outage {
  start: number;
  end: number | null;
  samples_lost: number;
}

/**
 * Coupures déduites des relevés.
 *
 * ⚠️ `end` reste `null` quand la série se termine sur un échec : la coupure
 * dure peut-être encore. La fermer à l'instant de l'export inventerait une
 * heure de rétablissement.
 */
export function outages(samples: Sample[]): Outage[] {
  const out: Outage[] = [];
  let current: Outage | null = null;
  for (const s of samples) {
    // ⚠️ `=== false`, PAS `!s.alive` : la négation est vraie pour `null`, et
    // chaque mesure indéterminée aurait ouvert une coupure. Le faux 0 % serait
    // revenu par la porte de derrière, sous forme de pannes inventées.
    if (s.alive === false) {
      if (current) current.samples_lost += 1;
      else current = { start: s.timestamp, end: null, samples_lost: 1 };
    } else if (current) {
      current.end = s.timestamp;
      out.push(current);
      current = null;
    }
  }
  if (current) out.push(current);
  return out;
}

export interface Stats {
  /**
   * 🔴 `null` = aucun relevé DÉTERMINÉ sur la période : on ne sait pas.
   *
   * ⚠️ Le type interdit d'imprimer un pourcentage qu'on n'a pas. `0` affirmait
   * une panne totale, dans un classeur remis au client — pire que le faux
   * 100 % qu'on corrigeait.
   */
  uptime_pct: number | null;
  avg: number | null;
  min: number | null;
  max: number | null;
  p95: number | null;
  /** Relevés DÉTERMINÉS — le dénominateur de `uptime_pct`. */
  total: number;
  failed: number;
  /** Relevés sans verdict. Comptés et affichés, jamais tus. */
  undetermined: number;
}

/**
 * Délai d'attente d'un ping, côté sonde (`lanprobe-core/src/ping.rs`).
 *
 * ⚠️ Une latence au-delà est impossible : le ping aurait abandonné avant. Elle
 * vient d'une sonde suspendue — un portable endormi pendant la mesure compte
 * son sommeil comme temps de réponse. Relevé en production : 1 045 504 ms.
 * Dans un rapport remis à un client, un « pic à 17 minutes » est pire que dans
 * un graphe : il se discute en réunion.
 *
 * ⚠️ Seules la moyenne, le minimum, le maximum et le p95 l'ignorent. La
 * DISPONIBILITÉ n'est pas touchée : elle se calcule sur `alive`, et réécrire
 * après coup un `alive` enregistré changerait un pourcentage de SLA déjà remis.
 */
export const PING_TIMEOUT_MS = 1000;

/**
 * ⚠️ La disponibilité se calcule sur `alive`, jamais sur la présence d'une
 * latence : un hôte muet n'en écrit pas, et compter les latences donnerait
 * 100 % sur un hôte mort.
 */
export function stats(samples: Sample[]): Stats {
  // ⚠️ Ce qui n'a pas été mesuré SORT du dénominateur : ni disponible, ni en
  // panne. Une fenêtre sans le moindre verdict n'a pas de pourcentage.
  const determines = samples.filter((s) => s.alive != null);
  const alive = determines.filter((s) => s.alive);
  const lat = samples
    .map((s) => s.latency_ms)
    .filter((v): v is number => typeof v === 'number' && Number.isFinite(v))
    // ⚠️ Une latence au-delà du délai d'attente vient d'une sonde suspendue,
    // pas du réseau. Un seul de ces points suffit à faire d'une moyenne un
    // chiffre que le client contestera à juste titre.
    .filter((v) => v <= PING_TIMEOUT_MS)
    .sort((a, b) => a - b);
  const p95 = lat.length ? lat[Math.min(lat.length - 1, Math.floor(lat.length * 0.95))] : null;
  return {
    uptime_pct: determines.length ? (alive.length / determines.length) * 100 : null,
    avg: lat.length ? lat.reduce((a, b) => a + b, 0) / lat.length : null,
    min: lat.length ? lat[0] : null,
    max: lat.length ? lat[lat.length - 1] : null,
    p95,
    total: determines.length,
    failed: determines.length - alive.length,
    undetermined: samples.length - determines.length,
  };
}

/** Durée d'une fenêtre Flux, en toutes lettres. */
/**
 * Traduction, telle que le rapport la consomme.
 *
 * Le rapport part chez un client : il doit être dans SA langue, pas dans celle
 * du code. On ne dépend donc pas d'un catalogue en dur.
 */
export type Translate = (key: string, values?: Record<string, string | number>) => string;

export function windowLabel(range: string, t: Translate, locale = 'en'): string {
  // Bornes explicites : « du … au … », qui est la seule forme qui vaille dans
  // un document contractuel.
  const bounds = /^(\d+)\.\.(\d+)$/.exec(range);
  if (bounds) {
    const fmt = (s: number) =>
      new Intl.DateTimeFormat(locale, { dateStyle: 'medium' }).format(new Date(s * 1000));
    return `${t('sla.from')} ${fmt(Number(bounds[1]))} ${t('sla.to')} ${fmt(Number(bounds[2]))}`;
  }
  const m = /^-(\d+)([hd])$/.exec(range);
  if (!m) return range;
  const n = Number(m[1]);
  return m[2] === 'd' ? t('sla.window_days', { n }) : t('sla.window_hours', { n });
}

/**
 * Graphique de latence d'une cible, en PNG.
 *
 * ⚠️ Dessiné sur un canvas plutôt qu'en graphique Excel natif : un graphique
 * natif se recalcule à l'ouverture, et un client qui trie une colonne le voit
 * changer sous ses yeux. Une image dit ce qu'on a mesuré, définitivement.
 *
 * Les coupures sont peintes en fond, pas seulement absentes de la courbe :
 * une ligne qui s'interrompt se confond avec une ligne qui sort du cadre.
 */
function chartPng(samples: Sample[], t: Translate, locale: string): string | null {
  if (samples.length < 2) return null;
  // Hors navigateur (tests du classeur), il n'y a pas de canvas. Le rapport se
  // lit très bien sans l'image : elle illustre des chiffres qui sont déjà là.
  if (typeof document === 'undefined') return null;
  const W = 900;
  const H = 260;
  const PAD = { top: 16, right: 16, bottom: 26, left: 52 };
  const canvas = document.createElement('canvas');
  canvas.width = W;
  canvas.height = H;
  const ctx = canvas.getContext('2d');
  if (!ctx) return null;

  const t0 = samples[0].timestamp;
  const t1 = samples[samples.length - 1].timestamp;
  const span = Math.max(1, t1 - t0);
  const lat = samples
    .map((s) => s.latency_ms)
    .filter((v): v is number => typeof v === 'number' && Number.isFinite(v));
  const vMax = Math.max(1, ...lat) * 1.15;
  const plotW = W - PAD.left - PAD.right;
  const plotH = H - PAD.top - PAD.bottom;
  const x = (ts: number) => PAD.left + ((ts - t0) / span) * plotW;
  const y = (v: number) => PAD.top + plotH - (v / vMax) * plotH;

  ctx.fillStyle = '#ffffff';
  ctx.fillRect(0, 0, W, H);

  // Coupures en fond.
  ctx.fillStyle = 'rgba(239, 68, 68, 0.16)';
  for (const o of outages(samples)) {
    const from = x(o.start);
    const to = x(o.end ?? t1);
    ctx.fillRect(from, PAD.top, Math.max(1, to - from), plotH);
  }

  // Grille et graduations.
  ctx.strokeStyle = '#e5e7eb';
  ctx.fillStyle = '#6b7280';
  ctx.font = '11px sans-serif';
  ctx.lineWidth = 1;
  for (let i = 0; i <= 4; i++) {
    const v = (vMax / 4) * i;
    const yy = Math.round(y(v)) + 0.5;
    ctx.beginPath();
    ctx.moveTo(PAD.left, yy);
    ctx.lineTo(W - PAD.right, yy);
    ctx.stroke();
    ctx.fillText(v.toFixed(0), 6, yy + 4);
  }

  // Courbe, coupée sur les interruptions — une droite au travers d'un trou
  // affirmerait une latence pendant une période sans mesure.
  ctx.strokeStyle = '#4f46e5';
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  let pen = false;
  for (const s of samples) {
    if (!s.alive || typeof s.latency_ms !== 'number') {
      pen = false;
      continue;
    }
    const px = x(s.timestamp);
    const py = y(s.latency_ms);
    if (pen) ctx.lineTo(px, py);
    else ctx.moveTo(px, py);
    pen = true;
  }
  ctx.stroke();

  // ⚠️ L'axe des temps, sans quoi une courbe ne dit que « ça a bougé » : on ne
  // sait ni quand la coupure a eu lieu, ni combien de temps couvre le
  // graphique. Dans un rapport remis à un client, c'est précisément la
  // question qu'il posera.
  const fmt = (ts: number) =>
    new Intl.DateTimeFormat(locale, {
      // Au-delà de 36 h, l'heure seule devient ambiguë : deux points à 14:00
      // peuvent être à deux jours d'écart.
      ...(span > 36 * 3600
        ? { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' }
        : { hour: '2-digit', minute: '2-digit' }),
    }).format(new Date(ts * 1000));

  ctx.fillStyle = '#6b7280';
  ctx.textAlign = 'center';
  const ticks = 5;
  for (let i = 0; i < ticks; i++) {
    const ts = t0 + (span * i) / (ticks - 1);
    const px = Math.min(W - PAD.right - 30, Math.max(PAD.left + 30, x(ts)));
    ctx.fillText(fmt(ts), px, H - 8);
  }
  ctx.textAlign = 'left';

  ctx.fillStyle = '#374151';
  ctx.fillText(t('sla.col_latency'), PAD.left, 11);

  return canvas.toDataURL('image/png').split(',')[1];
}

/**
 * Ajuste chaque colonne au plus long contenu qu'elle porte.
 *
 * ⚠️ Sans ça, une adresse longue ou un nom de serveur Ookla s'affichent en
 * `####` ou tronqués, et le lecteur doit élargir à la main — dans un document
 * qu'on lui a remis. Des largeurs fixes ne peuvent pas prévoir ce qu'un client
 * a comme noms d'hôtes.
 *
 * Plafonné : une URL de résultat ferait une colonne de 300 caractères qui
 * pousse tout le reste hors de l'écran.
 */
function autoFit(ws: import('exceljs').Worksheet, max = 46): void {
  ws.columns?.forEach((col) => {
    let widest = 10;
    col.eachCell?.({ includeEmpty: false }, (cell) => {
      const v = cell.value;
      const text =
        v == null
          ? ''
          : typeof v === 'object' && 'richText' in v
            ? String((v as { richText: { text: string }[] }).richText.map((r) => r.text).join(''))
            : String(v);
      // +2 : la bordure de cellule mange un peu de place, et un texte qui
      // touche exactement le bord se lit comme s'il était coupé.
      widest = Math.max(widest, Math.min(max, text.length + 2));
    });
    col.width = widest;
  });
}

function dt(seconds: number, locale: string): string {
  return new Intl.DateTimeFormat(locale, { dateStyle: 'short', timeStyle: 'medium' }).format(
    new Date(seconds * 1000),
  );
}

function duration(seconds: number, t: Translate): string {
  if (seconds < 60) return t('sla.dur_s', { n: seconds });
  if (seconds < 3600) return t('sla.dur_m', { n: Math.round(seconds / 60) });
  return t('sla.dur_h', { n: (seconds / 3600).toFixed(1) });
}

/**
 * Construit le classeur et déclenche son téléchargement.
 *
 * `t` est la fonction de traduction : le rapport part chez un client, il doit
 * être dans SA langue, pas dans celle du code.
 */
export async function downloadSlaReport(
  payload: SlaPayload,
  t: Translate,
  locale: string,
): Promise<void> {
  return downloadSlaWorkbook([payload], payload.probe, t, locale);
}

/**
 * Rapport couvrant **plusieurs sondes** — celui qu'on remet à un client qui
 * possède un site entier.
 *
 * ⚠️ Une seule synthèse en tête, toutes sondes confondues, avec une colonne
 * « sonde ». Un classeur d'une feuille par sonde obligerait le lecteur à
 * recomposer lui-même la vue d'ensemble, qui est justement ce qu'il ouvre le
 * document pour voir.
 */
export async function downloadSlaWorkbook(
  payloads: SlaPayload[],
  title: string,
  t: Translate,
  locale: string,
): Promise<void> {
  if (payloads.length === 0) return;
  const first = payloads[0];
  const wb = await buildSlaWorkbook(payloads, t, locale);

  const buffer = await wb.xlsx.writeBuffer();
  const blob = new Blob([buffer], {
    type: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  const slug = title.replace(/[^a-z0-9]+/gi, '-').replace(/^-|-$/g, '').toLowerCase();
  a.href = url;
  // Le nom de fichier porte la période : trois rapports du même site dans un
  // dossier de téléchargements doivent se distinguer sans les ouvrir.
  const period = first.range.includes('..')
    ? first.range.split('..').map((s) => new Date(Number(s) * 1000).toISOString().slice(0, 10)).join('_')
    : first.range.replace('-', '');
  a.download = `lanprobe-sla-${slug || 'rapport'}-${period}.xlsx`;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}

/**
 * Construit le classeur, sans le télécharger.
 *
 * ⚠️ Séparée du téléchargement pour une raison précise : `Blob`, `<a download>`
 * et `URL.createObjectURL` n'existent que dans un navigateur, donc un classeur
 * construit et téléchargé d'un seul tenant ne peut pas être vérifié. Le contenu
 * d'un document remis à un client est précisément ce qu'on veut pouvoir
 * vérifier. `downloadSlaWorkbook` reste le point d'entrée.
 */
export async function buildSlaWorkbook(
  payloads: SlaPayload[],
  t: Translate,
  locale: string,
): Promise<import('exceljs').Workbook> {
  // Import à la demande : exceljs pèse lourd, et la plupart des visites du hub
  // ne produisent aucun rapport.
  const ExcelJS = (await import('exceljs')).default;
  const wb = new ExcelJS.Workbook();
  wb.creator = 'LanProbe';
  if (payloads.length === 0) return wb;
  const first = payloads[0];
  wb.created = new Date(first.generated_at * 1000);

  const window = windowLabel(first.range, t, locale);
  const header = (ws: import('exceljs').Worksheet, probe?: string) => {
    if (probe) ws.addRow([t('sla.probe'), probe]);
    ws.addRow([t('sla.site'), first.site]);
    ws.addRow([t('sla.window'), window]);
    ws.addRow([t('sla.generated'), dt(first.generated_at, locale)]);
    ws.addRow([]);
  };

  interface Line {
    probe: string;
    label: string;
    samples: Sample[];
    coverage?: Coverage;
  }
  const rows: Line[] = payloads.flatMap((p) => [
    ...(p.internet.length ? [{ probe: p.probe, label: t('sla.internet'), samples: p.internet }] : []),
    ...p.targets.map((tg) => ({
      probe: p.probe,
      label: tg.ip,
      samples: tg.samples,
      coverage: tg.coverage,
    })),
  ]);

  // ── Synthèse ────────────────────────────────────────────────────────────
  const sum = wb.addWorksheet(t('sla.sheet_summary'));
  header(sum, payloads.length === 1 ? first.probe : undefined);
  sum.addRow([
    t('sla.probe'),
    t('sla.col_target'),
    t('sla.col_uptime'),
    t('sla.col_avg'),
    t('sla.col_min'),
    t('sla.col_max'),
    t('sla.col_p95'),
    t('sla.col_samples'),
    t('sla.col_outages'),
    t('sla.col_coverage'),
  ]);

  for (const r of rows) {
    const s = stats(r.samples);
    sum.addRow([
      r.probe,
      r.label,
      // ⚠️ Le mot, pas une cellule vide (qui se lit comme un oubli de l'outil)
      // ni `0` (qui se lit comme une panne). ⚠️ Et une CHAÎNE, pas un nombre :
      // une moyenne de colonne dans le tableur ne doit pas l'agréger comme un
      // zéro — c'est exactement là que le faux 0 % reviendrait.
      s.uptime_pct != null ? +s.uptime_pct.toFixed(2) : t('sla.undetermined'),
      s.avg != null ? +s.avg.toFixed(1) : '—',
      s.min ?? '—',
      s.max ?? '—',
      s.p95 ?? '—',
      s.total,
      outages(r.samples).length,
      // ⚠️ Une chaîne, jamais un nombre : un total ou une moyenne de colonne
      // ne doit pas pouvoir l'aspirer. Vide quand la période est entièrement
      // mesurée — c'est le cas normal, et le dire serait du bruit.
      coverageLabel(r.coverage, t) ?? '',
    ]);
  }
  autoFit(sum);

  // ── Une feuille par cible : incidents puis série ─────────────────────────
  const used = new Set<string>();
  for (const r of rows) {
    // Excel refuse ces caractères dans un nom d'onglet, plafonne à 31
    // caractères et rejette un doublon — deux sondes qui surveillent la même
    // adresse, c'est le cas courant. Le classeur entier échouerait à
    // l'écriture sur n'importe lequel des trois.
    const base = (payloads.length > 1 ? `${r.probe} ${r.label}` : r.label)
      .replace(/[\\/*?:[\]]/g, '-')
      .slice(0, 31);
    let name = base;
    for (let n = 2; used.has(name); n++) name = `${base.slice(0, 28)}~${n}`;
    used.add(name);

    const ws = wb.addWorksheet(name);
    header(ws, r.probe);

    const s = stats(r.samples);
    ws.addRow([
      t('sla.col_uptime'),
      s.uptime_pct != null ? +s.uptime_pct.toFixed(2) + ' %' : t('sla.undetermined'),
    ]);
    ws.addRow([t('sla.col_samples'), s.total]);
    ws.addRow([t('sla.col_failed'), s.failed]);
    // Le compte des indéterminés voyage avec le chiffre : « 100 % sur trois
    // relevés » et « 100 % sur trois relevés et neuf mille indéterminés » ne
    // se lisent pas pareil.
    ws.addRow([t('sla.col_undetermined'), s.undetermined]);
    const couverture = coverageLabel(r.coverage, t);
    if (couverture) ws.addRow([t('sla.col_coverage'), couverture]);
    ws.addRow([]);

    ws.addRow([t('sla.sheet_outages')]);
    ws.addRow([t('sla.col_start'), t('sla.col_end'), t('sla.col_duration'), t('sla.col_lost')]);
    const list = outages(r.samples);
    if (list.length === 0) {
      ws.addRow([t('sla.no_outage')]);
    } else {
      for (const o of list) {
        ws.addRow([
          dt(o.start, locale),
          // ⚠️ Jamais une heure de fin inventée : une coupure encore en cours
          // se dit, elle ne se clôt pas sur l'instant de l'export.
          o.end != null ? dt(o.end, locale) : t('sla.ongoing'),
          o.end != null ? duration(o.end - o.start, t) : t('sla.ongoing'),
          o.samples_lost,
        ]);
      }
    }
    ws.addRow([]);

    ws.addRow([t('sla.sheet_series')]);
    ws.addRow([t('sla.col_time'), t('sla.col_state'), t('sla.col_latency')]);
    for (const sample of r.samples) {
      ws.addRow([
        dt(sample.timestamp, locale),
        sample.state ?? (sample.alive ? 'OK' : 'KO'),
        sample.latency_ms ?? '',
      ]);
    }
    autoFit(ws);

    // Le graphique se pose à droite des données, jamais dessus : il ne doit
    // pas masquer les chiffres qu'il illustre.
    const png = chartPng(r.samples, t, locale);
    if (png) {
      const imgId = wb.addImage({ base64: png, extension: 'png' });
      ws.addImage(imgId, {
        tl: { col: 5, row: 1 } as never,
        ext: { width: 720, height: 210 },
      });
    }
  }

  // ── Débits ──────────────────────────────────────────────────────────────
  const speedRows = payloads.flatMap((p) =>
    p.speedtests.map((r) => ({ probe: p.probe, ...r })),
  );
  if (speedRows.length) {
    const ws = wb.addWorksheet(t('sla.sheet_speed'));
    header(ws, payloads.length === 1 ? first.probe : undefined);
    ws.addRow([
      t('sla.probe'),
      t('sla.col_time'),
      t('sla.col_engine'),
      t('sla.col_server'),
      t('sla.col_down'),
      t('sla.col_up'),
      t('sla.col_latency'),
      t('sla.col_jitter'),
    ]);
    for (const r of speedRows) {
      ws.addRow([
        r.probe,
        dt(r.started_at, locale),
        r.engine ?? '—',
        r.server_name ?? '—',
        r.download_mbps != null ? +r.download_mbps.toFixed(1) : '—',
        r.upload_mbps != null ? +r.upload_mbps.toFixed(1) : '—',
        r.latency_ms ?? '—',
        r.jitter_ms != null ? +r.jitter_ms.toFixed(1) : '—',
      ]);
    }
    autoFit(ws);
  }

  // ── Inventaire : machines vues, et ports ouverts ────────────────────────
  //
  // Un client qui reçoit un SLA veut aussi savoir ce qui tournait chez lui.
  const hosts = payloads.flatMap((p) =>
    (p.discovery?.hosts ?? []).map((h) => ({ probe: p.probe, at: p.discovery!.started_at, ...h })),
  );
  if (hosts.length) {
    const ws = wb.addWorksheet(t('sla.sheet_hosts'));
    header(ws, payloads.length === 1 ? first.probe : undefined);
    ws.addRow([
      t('sla.probe'),
      t('sla.col_scan_at'),
      t('sla.col_ip'),
      t('sla.col_hostname'),
      t('sla.col_mac'),
      t('sla.col_vendor'),
      t('sla.col_latency'),
    ]);
    for (const h of hosts) {
      ws.addRow([
        h.probe,
        dt(h.at, locale),
        h.ip,
        h.hostname ?? '—',
        h.mac ?? '—',
        h.vendor ?? '—',
        h.latency_ms ?? '—',
      ]);
    }
    autoFit(ws);
  }

  const openPorts = payloads.flatMap((p) =>
    (p.ports?.ports ?? []).map((o) => ({ probe: p.probe, at: p.ports!.started_at, ...o })),
  );
  if (openPorts.length) {
    const ws = wb.addWorksheet(t('sla.sheet_ports'));
    header(ws, payloads.length === 1 ? first.probe : undefined);
    ws.addRow([
      t('sla.probe'),
      t('sla.col_scan_at'),
      t('sla.col_ip'),
      t('sla.col_port'),
      t('sla.col_proto'),
      t('sla.col_service'),
    ]);
    for (const o of openPorts) {
      ws.addRow([o.probe, dt(o.at, locale), o.ip, o.port, o.proto, o.service ?? '—']);
    }
    autoFit(ws);
  }

  // ── SLA par IP publique ─────────────────────────────────────────────────
  //
  // Un client qui conteste un SLA veut savoir sur quel lien il a été mesuré.
  //
  // ⚠️ Le découpage se fait sonde par sonde : les intervalles d'une sonde ne
  // décrivent que son propre lien, et croiser les relevés de l'une avec les
  // intervalles de l'autre imputerait une coupure à une adresse qui n'y était
  // pour rien.
  const ipRows = payloads.flatMap((p) =>
    byPublicIp(p.internet, p.public_ip_history ?? []).map((r) => ({ probe: p.probe, ...r })),
  );
  if (ipRows.length) {
    const ws = wb.addWorksheet(t('report.ipSheet'));
    ws.addRow([
      t('sla.probe'),
      t('report.ipLabel'),
      t('report.ipAddress'),
      t('report.ipGateway'),
      t('report.ipFrom'),
      t('report.ipTo'),
      t('report.ipSamples'),
      t('report.ipUptime'),
    ]);
    for (const r of ipRows) {
      ws.addRow([
        r.probe,
        r.label ?? '',
        // ⚠️ La ligne indéterminée n'est pas une adresse manquante : c'est une
        // part de la période qu'on refuse d'imputer. Elle doit se lire comme
        // telle, pas comme une case vide.
        r.public_ip ?? t('report.ipUndetermined'),
        r.gateway ?? '',
        r.from ? dt(r.from, locale) : '',
        r.to ? dt(r.to, locale) : '',
        r.samples,
        r.uptime_pct != null ? `${r.uptime_pct.toFixed(2)} %` : t('sla.undetermined'),
      ]);
    }
    autoFit(ws);
  }

  return wb;
}

export interface PublicIpInterval {
  public_ip: string;
  interface: string | null;
  gateway: string | null;
  local_subnet: string | null;
  confirmed_from: number;
  confirmed_until: number;
  label: string | null;
}

export interface IpSlaRow {
  /** `null` = indéterminé : aucun intervalle ne couvrait ces relevés. */
  public_ip: string | null;
  label: string | null;
  gateway: string | null;
  from: number | null;
  to: number | null;
  samples: number;
  /** `null` = aucun relevé déterminé sur cette tranche. */
  uptime_pct: number | null;
}

/**
 * Répartit les relevés d'accès internet par adresse publique.
 *
 * ⚠️ Les pourcentages ne totalisent PAS 100 % : un relevé qu'aucun intervalle
 * ne couvre va en « indéterminé » et n'est imputé à personne. C'est délibéré.
 * Une coupure attribuée à la mauvaise adresse a l'air normale et personne ne la
 * remet en cause — c'est exactement le mode de panne que le projet combat
 * partout ailleurs.
 *
 * ⚠️ Tout est en **secondes** ici, des deux côtés de la comparaison : les
 * relevés comme les bornes d'intervalle viennent du hub. La conversion en
 * millisecondes n'a lieu qu'au passage au graphe, jamais ici.
 */
export function byPublicIp(samples: Sample[], intervals: PublicIpInterval[]): IpSlaRow[] {
  const ordered = [...intervals].sort((a, b) => a.confirmed_from - b.confirmed_from);
  const buckets = new Map<string, { meta: PublicIpInterval | null; samples: Sample[] }>();

  const push = (key: string, meta: PublicIpInterval | null, s: Sample) => {
    const b = buckets.get(key) ?? { meta, samples: [] };
    b.samples.push(s);
    buckets.set(key, b);
  };

  for (const s of samples) {
    const hit = ordered.find(
      (i) => s.timestamp >= i.confirmed_from && s.timestamp <= i.confirmed_until,
    );
    if (hit) push(hit.public_ip, hit, s);
    else push('', null, s);
  }

  const rows: IpSlaRow[] = [];
  for (const [key, bucket] of buckets) {
    const times = bucket.samples.map((s) => s.timestamp);
    rows.push({
      public_ip: key === '' ? null : key,
      label: bucket.meta?.label ?? null,
      gateway: bucket.meta?.gateway ?? null,
      from: times.length ? Math.min(...times) : null,
      to: times.length ? Math.max(...times) : null,
      samples: bucket.samples.length,
      uptime_pct: stats(bucket.samples).uptime_pct,
    });
  }
  // L'indéterminé en dernier : c'est un reste, pas une ligne comme les autres.
  return rows.sort((a, b) => {
    if (a.public_ip === null) return 1;
    if (b.public_ip === null) return -1;
    return (a.from ?? 0) - (b.from ?? 0);
  });
}
