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
  alive: boolean;
  latency_ms?: number | null;
  /** Présent pour l'accès internet : `online` / `limited` / `offline`. */
  state?: string;
}

export interface TargetSeries {
  ip: string;
  samples: Sample[];
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

export interface SlaPayload {
  probe: string;
  site: string;
  range: string;
  generated_at: number;
  targets: TargetSeries[];
  internet: Sample[];
  speedtests: SpeedtestRow[];
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
    if (!s.alive) {
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
  uptime_pct: number;
  avg: number | null;
  min: number | null;
  max: number | null;
  p95: number | null;
  total: number;
  failed: number;
}

/**
 * ⚠️ La disponibilité se calcule sur `alive`, jamais sur la présence d'une
 * latence : un hôte muet n'en écrit pas, et compter les latences donnerait
 * 100 % sur un hôte mort.
 */
export function stats(samples: Sample[]): Stats {
  if (samples.length === 0) {
    return { uptime_pct: 0, avg: null, min: null, max: null, p95: null, total: 0, failed: 0 };
  }
  const alive = samples.filter((s) => s.alive);
  const lat = samples
    .map((s) => s.latency_ms)
    .filter((v): v is number => typeof v === 'number' && Number.isFinite(v))
    .sort((a, b) => a - b);
  const p95 = lat.length ? lat[Math.min(lat.length - 1, Math.floor(lat.length * 0.95))] : null;
  return {
    uptime_pct: (alive.length / samples.length) * 100,
    avg: lat.length ? lat.reduce((a, b) => a + b, 0) / lat.length : null,
    min: lat.length ? lat[0] : null,
    max: lat.length ? lat[lat.length - 1] : null,
    p95,
    total: samples.length,
    failed: samples.length - alive.length,
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

export function windowLabel(range: string, t: Translate): string {
  const m = /^-(\d+)([hd])$/.exec(range);
  if (!m) return range;
  const n = Number(m[1]);
  return m[2] === 'd' ? t('sla.window_days', { n }) : t('sla.window_hours', { n });
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
  // Import à la demande : exceljs pèse lourd, et la plupart des visites du hub
  // ne produisent aucun rapport.
  const ExcelJS = (await import('exceljs')).default;
  const wb = new ExcelJS.Workbook();
  wb.creator = 'LanProbe';
  wb.created = new Date(payload.generated_at * 1000);

  const window = windowLabel(payload.range, t);
  const header = (ws: import('exceljs').Worksheet) => {
    ws.addRow([t('sla.probe'), payload.probe]);
    ws.addRow([t('sla.site'), payload.site]);
    ws.addRow([t('sla.window'), window]);
    ws.addRow([t('sla.generated'), dt(payload.generated_at, locale)]);
    ws.addRow([]);
  };

  // ── Synthèse ────────────────────────────────────────────────────────────
  const sum = wb.addWorksheet(t('sla.sheet_summary'));
  header(sum);
  sum.addRow([
    t('sla.col_target'),
    t('sla.col_uptime'),
    t('sla.col_avg'),
    t('sla.col_min'),
    t('sla.col_max'),
    t('sla.col_p95'),
    t('sla.col_samples'),
    t('sla.col_outages'),
  ]);

  const rows: { label: string; samples: Sample[] }[] = [
    ...(payload.internet.length ? [{ label: t('sla.internet'), samples: payload.internet }] : []),
    ...payload.targets.map((tg) => ({ label: tg.ip, samples: tg.samples })),
  ];

  for (const r of rows) {
    const s = stats(r.samples);
    sum.addRow([
      r.label,
      +s.uptime_pct.toFixed(2),
      s.avg != null ? +s.avg.toFixed(1) : '—',
      s.min ?? '—',
      s.max ?? '—',
      s.p95 ?? '—',
      s.total,
      outages(r.samples).length,
    ]);
  }
  sum.columns = [22, 12, 12, 10, 10, 10, 12, 12].map((width) => ({ width }));

  // ── Une feuille par cible : incidents puis série ─────────────────────────
  for (const r of rows) {
    // Excel refuse ces caractères dans un nom d'onglet, et une adresse IPv6
    // en contient. Sans ce nettoyage, le classeur entier échoue à l'écriture.
    const name = r.label.replace(/[\\/*?:[\]]/g, '-').slice(0, 31);
    const ws = wb.addWorksheet(name);
    header(ws);

    const s = stats(r.samples);
    ws.addRow([t('sla.col_uptime'), +s.uptime_pct.toFixed(2) + ' %']);
    ws.addRow([t('sla.col_samples'), s.total]);
    ws.addRow([t('sla.col_failed'), s.failed]);
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
    ws.columns = [24, 14, 14, 12].map((width) => ({ width }));
  }

  // ── Débits ──────────────────────────────────────────────────────────────
  if (payload.speedtests.length) {
    const ws = wb.addWorksheet(t('sla.sheet_speed'));
    header(ws);
    ws.addRow([
      t('sla.col_time'),
      t('sla.col_engine'),
      t('sla.col_server'),
      t('sla.col_down'),
      t('sla.col_up'),
      t('sla.col_latency'),
      t('sla.col_jitter'),
    ]);
    for (const r of payload.speedtests) {
      ws.addRow([
        dt(r.started_at, locale),
        r.engine ?? '—',
        r.server_name ?? '—',
        r.download_mbps != null ? +r.download_mbps.toFixed(1) : '—',
        r.upload_mbps != null ? +r.upload_mbps.toFixed(1) : '—',
        r.latency_ms ?? '—',
        r.jitter_ms != null ? +r.jitter_ms.toFixed(1) : '—',
      ]);
    }
    ws.columns = [24, 12, 30, 12, 12, 12, 12].map((width) => ({ width }));
  }

  const buffer = await wb.xlsx.writeBuffer();
  const blob = new Blob([buffer], {
    type: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  const slug = payload.probe.replace(/[^a-z0-9]+/gi, '-').replace(/^-|-$/g, '').toLowerCase();
  a.href = url;
  a.download = `lanprobe-sla-${slug || 'sonde'}-${payload.range.replace('-', '')}.xlsx`;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}
