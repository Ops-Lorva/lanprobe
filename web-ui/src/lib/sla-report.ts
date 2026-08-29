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
function chartPng(samples: Sample[], t: Translate): string | null {
  if (samples.length < 2) return null;
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

  ctx.fillStyle = '#374151';
  ctx.fillText(t('sla.col_latency'), PAD.left, 11);

  return canvas.toDataURL('image/png').split(',')[1];
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
  // Import à la demande : exceljs pèse lourd, et la plupart des visites du hub
  // ne produisent aucun rapport.
  const ExcelJS = (await import('exceljs')).default;
  const wb = new ExcelJS.Workbook();
  wb.creator = 'LanProbe';
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
  }
  const rows: Line[] = payloads.flatMap((p) => [
    ...(p.internet.length ? [{ probe: p.probe, label: t('sla.internet'), samples: p.internet }] : []),
    ...p.targets.map((tg) => ({ probe: p.probe, label: tg.ip, samples: tg.samples })),
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
  ]);

  for (const r of rows) {
    const s = stats(r.samples);
    sum.addRow([
      r.probe,
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
  sum.columns = [18, 22, 12, 12, 10, 10, 10, 12, 12].map((width) => ({ width }));

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

    // Le graphique se pose à droite des données, jamais dessus : il ne doit
    // pas masquer les chiffres qu'il illustre.
    const png = chartPng(r.samples, t);
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
    ws.columns = [18, 24, 12, 30, 12, 12, 12, 12].map((width) => ({ width }));
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
    ws.columns = [18, 22, 16, 24, 20, 22, 12].map((width) => ({ width }));
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
    ws.columns = [18, 22, 16, 10, 10, 22].map((width) => ({ width }));
  }

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
