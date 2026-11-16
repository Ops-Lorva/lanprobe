<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { _ } from 'svelte-i18n';
  import { get } from 'svelte/store';
  import { monitoring, type HostMonitor } from '../stores/monitoring';
  import { internetStatus } from '../stores/internetStatus';
  import { selectedInterface } from '../stores/selectedInterface';
  import ExcelJS from 'exceljs';
  import { locale } from 'svelte-i18n';
  import ScreenHead from './ScreenHead.svelte';
  import StateMark from './StateMark.svelte';
  import LatencySpark from './LatencySpark.svelte';
  import LatencyChart from './LatencyChart.svelte';
  import { internetMark, targetMark } from '../status';
  import { fmtNum } from '../format';
  import { screenKind } from '../screen';

  // Dans une case de la grille (une page) : tableau resserré, sans courbe.
  const isCell = screenKind() === 'cell';
  const lang = $derived($locale ?? 'en');
  /** La cible dont la courbe s'affiche sous le tableau. */
  let selectedIp = $state<string | null>(null);

  let newIp = $state('');
  let adding = $state(false);

  // ── SLA Export ──────────────────────────────────────────────────────────
  let showExport = $state(false);
  let exportSelected = $state<Set<string>>(new Set());
  let exportIncludeInternet = $state(true);
  let exporting = $state(false);

  function openExport() {
    exportSelected = new Set([...$monitoring.keys()]);
    exportIncludeInternet = true;
    showExport = true;
  }

  function toggleExportIp(ip: string) {
    const s = new Set(exportSelected);
    if (s.has(ip)) s.delete(ip); else s.add(ip);
    exportSelected = s;
  }

  function exportSelectAll() {
    exportSelected = new Set([...$monitoring.keys()]);
    exportIncludeInternet = true;
  }

  function exportDeselectAll() {
    exportSelected = new Set();
    exportIncludeInternet = false;
  }

  type Sample = { alive: boolean; latency_ms: number | null; timestamp: number };

  interface Outage {
    start: number;   // unix seconds
    end: number | null;
    samples_lost: number;
  }

  function computeOutages(samples: Sample[]): Outage[] {
    const out: Outage[] = [];
    let start: number | null = null;
    let lost = 0;
    for (const s of samples) {
      if (!s.alive) {
        if (start === null) { start = s.timestamp; lost = 0; }
        lost++;
      } else if (start !== null) {
        out.push({ start, end: s.timestamp, samples_lost: lost });
        start = null; lost = 0;
      }
    }
    if (start !== null) out.push({ start, end: null, samples_lost: lost });
    return out;
  }

  function localDt(ts: number): string {
    return new Intl.DateTimeFormat(undefined, {
      year: 'numeric', month: '2-digit', day: '2-digit',
      hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false
    }).format(new Date(ts * 1000));
  }

  const CHART_COLORS = ['#3b82f6', '#10b981', '#f59e0b', '#8b5cf6', '#06b6d4', '#f97316'];

  function renderChartPng(sources: { label: string; samples: Sample[] }[], nowTs: number): string {
    const W = 900, H = 420;
    const PAD = { top: 50, right: 30, bottom: 55, left: 65 };
    const PW = W - PAD.left - PAD.right;
    const PH = H - PAD.top - PAD.bottom;

    const canvas = document.createElement('canvas');
    canvas.width = W; canvas.height = H;
    const ctx = canvas.getContext('2d')!;

    ctx.fillStyle = '#ffffff';
    ctx.fillRect(0, 0, W, H);

    const allSamples = sources.flatMap(s => s.samples);
    if (allSamples.length === 0) {
      ctx.fillStyle = '#6b7280';
      ctx.font = '14px sans-serif';
      ctx.textAlign = 'center';
      ctx.fillText(get(_)('monitoring.xlsx_no_data') as string, W / 2, H / 2);
      return canvas.toDataURL('image/png').split(',')[1];
    }

    const minTs = Math.min(...allSamples.map(s => s.timestamp));
    const maxTs = Math.max(...allSamples.map(s => s.timestamp), minTs + 1);
    const tsRange = maxTs - minTs;
    const allLats = allSamples.filter(s => s.latency_ms != null).map(s => s.latency_ms as number);
    const maxLat = allLats.length ? Math.max(...allLats) * 1.15 : 100;

    const toX = (ts: number) => PAD.left + ((ts - minTs) / tsRange) * PW;
    const toY = (ms: number) => PAD.top + PH - (ms / maxLat) * PH;

    // grid horizontales
    ctx.lineWidth = 1;
    for (let i = 0; i <= 5; i++) {
      const ms = (maxLat / 5) * i;
      const y = toY(ms);
      ctx.strokeStyle = i === 0 ? '#9ca3af' : '#e5e7eb';
      ctx.beginPath(); ctx.moveTo(PAD.left, y); ctx.lineTo(PAD.left + PW, y); ctx.stroke();
      ctx.fillStyle = '#6b7280'; ctx.font = '11px sans-serif'; ctx.textAlign = 'right';
      ctx.fillText(Math.round(ms) + ' ms', PAD.left - 8, y + 4);
    }

    // grid verticales + labels X
    for (let i = 0; i <= 6; i++) {
      const ts = minTs + tsRange * (i / 6);
      const x = toX(ts);
      ctx.strokeStyle = '#e5e7eb'; ctx.lineWidth = 1;
      ctx.beginPath(); ctx.moveTo(x, PAD.top); ctx.lineTo(x, PAD.top + PH); ctx.stroke();
      ctx.fillStyle = '#6b7280'; ctx.font = '11px sans-serif'; ctx.textAlign = 'center';
      const lbl = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false })
        .format(new Date(ts * 1000));
      ctx.fillText(lbl, x, PAD.top + PH + 20);
    }

    // zones coupures
    for (const { samples } of sources) {
      for (const o of computeOutages(samples)) {
        const x1 = toX(o.start);
        const x2 = toX(o.end ?? nowTs);
        ctx.fillStyle = 'rgba(239,68,68,0.12)';
        ctx.fillRect(x1, PAD.top, Math.max(x2 - x1, 2), PH);
      }
    }

    // courbes latence
    for (let si = 0; si < sources.length; si++) {
      const color = CHART_COLORS[si % CHART_COLORS.length];
      const pts = sources[si].samples.filter(s => s.alive && s.latency_ms != null);
      if (pts.length < 2) continue;
      ctx.strokeStyle = color; ctx.lineWidth = 1.5; ctx.lineJoin = 'round';
      ctx.beginPath();
      pts.forEach((s, i) => {
        const x = toX(s.timestamp), y = toY(s.latency_ms as number);
        i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y);
      });
      ctx.stroke();
    }

    // bordure zone graphique
    ctx.strokeStyle = '#d1d5db'; ctx.lineWidth = 1;
    ctx.strokeRect(PAD.left, PAD.top, PW, PH);

    // titre
    ctx.fillStyle = '#111827'; ctx.font = 'bold 13px sans-serif'; ctx.textAlign = 'center';
    ctx.fillText(get(_)('monitoring.xlsx_col_latency') + ' — ' + localDt(minTs) + ' → ' + localDt(maxTs), W / 2, 22);

    // légende
    let lx = PAD.left;
    for (let si = 0; si < sources.length; si++) {
      const color = CHART_COLORS[si % CHART_COLORS.length];
      ctx.fillStyle = color; ctx.fillRect(lx, H - 20, 14, 10);
      ctx.fillStyle = '#374151'; ctx.font = '11px sans-serif'; ctx.textAlign = 'left';
      ctx.fillText(sources[si].label, lx + 18, H - 11);
      lx += 20 + ctx.measureText(sources[si].label).width + 18;
    }
    // légende zone rouge
    ctx.fillStyle = 'rgba(239,68,68,0.35)'; ctx.fillRect(lx, H - 20, 14, 10);
    ctx.fillStyle = '#374151'; ctx.font = '11px sans-serif'; ctx.textAlign = 'left';
    ctx.fillText(get(_)('monitoring.xlsx_outage') as string, lx + 18, H - 11);

    return canvas.toDataURL('image/png').split(',')[1];
  }

  function statsForSource(samples: Sample[], nowTs: number) {
    const total = samples.length;
    const failed = samples.filter(s => !s.alive).length;
    const up = total === 0 ? 100 : ((total - failed) / total) * 100;
    const lats = samples.filter(s => s.alive && s.latency_ms != null)
      .map(s => s.latency_ms as number).sort((a, b) => a - b);
    const avg = lats.length ? lats.reduce((a, b) => a + b, 0) / lats.length : null;
    const min = lats.length ? lats[0] : null;
    const max = lats.length ? lats[lats.length - 1] : null;
    const p95 = lats.length ? lats[Math.min(lats.length - 1, Math.ceil(lats.length * 0.95) - 1)] : null;
    const outages = computeOutages(samples);
    const longest = outages.reduce((acc, o) => {
      const dur = o.end != null ? o.end - o.start : nowTs - o.start;
      return Math.max(acc, dur);
    }, 0);
    const firstTs = samples.length ? localDt(samples[0].timestamp) : '—';
    return { total, failed, up, avg, min, max, p95, outages, longest, firstTs };
  }

  function addChartSheet(wb: ExcelJS.Workbook, sheetName: string, sources: { label: string; samples: Sample[] }[], nowTs: number) {
    const ws = wb.addWorksheet(sheetName);
    const pngB64 = renderChartPng(sources, nowTs);
    const imgId = wb.addImage({ base64: pngB64, extension: 'png' });
    ws.addImage(imgId, { tl: { col: 0, row: 0 } as any, ext: { width: 900, height: 420 } });
  }

  async function doExportXlsx() {
    if (exporting) return;
    exporting = true;
    try {
      const snap = await invoke<Record<string, Sample[]>>('cmd_get_monitoring_snapshot');
      const now = new Date();
      const nowTs = Math.floor(now.getTime() / 1000);
      const slug = now.toISOString().replace(/[:.]/g, '-').slice(0, 19);
      const iface = $selectedInterface || '—';
      const t = (k: string) => get(_)(k) as string;

      // extraSeries : séries supplémentaires affichées dans le graphique et le
      // tableau de la feuille détaillée, mais pas utilisées pour les stats SLA.
      type SourceData = { label: string; samples: Sample[]; extraSeries?: { label: string; samples: Sample[] }[] };
      const sources: SourceData[] = [];

      if (exportIncludeInternet) {
        const total = $internetStatus?.samples ?? internetHistory.length;
        const upPct = $internetStatus?.uptime_pct ?? 100;
        const failedCount = Math.round(total * (1 - upPct / 100));
        const fakeSamples: Sample[] = internetHistory.map((ms, i) => ({
          alive: true, latency_ms: ms,
          timestamp: nowTs - (internetHistory.length - i)
        }));
        if (failedCount > 0 && fakeSamples.length > 0) {
          const step = Math.max(1, Math.floor(fakeSamples.length / failedCount));
          for (let i = 0; i < fakeSamples.length; i += step) {
            fakeSamples[i] = { ...fakeSamples[i], alive: false, latency_ms: null };
          }
        }
        // DNS comme série supplémentaire — fusionné avec Ping dans une seule feuille
        const dnsSamples: Sample[] = internetDnsHistory.map((dns, i) => ({
          alive: dns != null,
          latency_ms: dns,
          timestamp: nowTs - (internetDnsHistory.length - i)
        }));
        const extra = dnsSamples.some(s => s.alive)
          ? [{ label: 'DNS', samples: dnsSamples }]
          : undefined;
        sources.push({ label: t('monitoring.internet'), samples: fakeSamples, extraSeries: extra });
      }
      for (const ip of exportSelected) {
        sources.push({ label: ip, samples: snap?.[ip] ?? [] });
      }

      const wb = new ExcelJS.Workbook();

      // ── Feuille Résumé global ─────────────────────────────────────────────
      const ws1 = wb.addWorksheet(t('monitoring.xlsx_sheet_summary'));
      ws1.addRow(['LanProbe — ' + t('monitoring.export_modal_title')]);
      ws1.addRow([t('monitoring.xlsx_exported'), localDt(nowTs)]);
      ws1.addRow([t('monitoring.xlsx_interface'), iface]);
      ws1.addRow([]);
      ws1.addRow([
        t('monitoring.xlsx_col_source'),
        t('monitoring.xlsx_col_since'),
        t('monitoring.xlsx_col_uptime'),
        t('monitoring.xlsx_col_avg'),
        t('monitoring.xlsx_col_min'),
        t('monitoring.xlsx_col_max'),
        t('monitoring.xlsx_col_p95'),
        t('monitoring.xlsx_col_samples'),
        t('monitoring.xlsx_col_failures'),
        t('monitoring.xlsx_col_outages'),
        t('monitoring.xlsx_col_worst'),
      ]);
      for (const { label, samples } of sources) {
        const s = statsForSource(samples, nowTs);
        ws1.addRow([label, s.firstTs, +s.up.toFixed(2),
          s.avg != null ? +s.avg.toFixed(1) : '',
          s.min ?? '', s.max ?? '', s.p95 ?? '',
          s.total, s.failed, s.outages.length, s.longest]);
      }
      ws1.columns = [18, 24, 10, 10, 10, 10, 10, 13, 8, 10, 16].map(w => ({ width: w }));

      // ── Graphique global ──────────────────────────────────────────────────
      // Toutes séries (primaires + DNS fusionné) dans le même graphique
      const allChartSeries = sources.flatMap(src => [
        { label: src.label, samples: src.samples },
        ...(src.extraSeries ?? []),
      ]);
      addChartSheet(wb, t('monitoring.xlsx_sheet_chart'), allChartSeries, nowTs);

      // ── Une feuille par source ────────────────────────────────────────────
      for (let si = 0; si < sources.length; si++) {
        const { label, samples, extraSeries } = sources[si];
        const sheetName = label.slice(0, 31);
        const ws = wb.addWorksheet(sheetName);
        const s = statsForSource(samples, nowTs);

        // Métadonnées (stats basées sur la série primaire = Ping pour Internet)
        ws.addRow([label + ' — ' + t('monitoring.export_modal_title')]);
        ws.addRow([t('monitoring.xlsx_col_since'), s.firstTs]);
        ws.addRow([t('monitoring.xlsx_col_uptime'), +s.up.toFixed(2) + ' %']);
        ws.addRow([t('monitoring.xlsx_col_avg'), s.avg != null ? +s.avg.toFixed(1) + ' ms' : '—']);
        ws.addRow([t('monitoring.xlsx_col_min') + ' / ' + t('monitoring.xlsx_col_max'),
          (s.min ?? '—') + ' / ' + (s.max ?? '—') + ' ms']);
        ws.addRow([t('monitoring.xlsx_col_p95'), (s.p95 ?? '—') + ' ms']);
        ws.addRow([t('monitoring.xlsx_col_outages'), s.outages.length]);
        ws.addRow([t('monitoring.xlsx_col_worst'), s.longest + ' s']);
        ws.addRow([]);

        // Incidents
        ws.addRow([t('monitoring.xlsx_sheet_incidents')]);
        ws.addRow([
          t('monitoring.xlsx_col_inc_start'),
          t('monitoring.xlsx_col_inc_end'),
          t('monitoring.xlsx_col_inc_dur'),
          t('monitoring.xlsx_col_inc_lost'),
          t('monitoring.xlsx_col_status'),
        ]);
        if (s.outages.length === 0) {
          ws.addRow([t('monitoring.xlsx_no_outage')]);
        } else {
          for (const o of s.outages) {
            const end = o.end ?? nowTs;
            ws.addRow([
              localDt(o.start),
              o.end != null ? localDt(o.end) : t('monitoring.xlsx_ongoing'),
              end - o.start,
              o.samples_lost,
              o.end != null ? t('monitoring.xlsx_restored') : t('monitoring.xlsx_ongoing'),
            ]);
          }
        }
        ws.addRow([]);

        // Série temporelle — colonnes Ping + DNS fusionnées si extraSeries présents
        ws.addRow([t('monitoring.xlsx_sheet_series')]);
        if (extraSeries?.length) {
          // Internet : colonnes Ping ms + DNS ms alignées sur le même timestamp
          ws.addRow([t('monitoring.xlsx_col_time'), t('monitoring.xlsx_col_state'), 'Ping (ms)', 'DNS (ms)']);
          const dnsSamples = extraSeries[0].samples;
          const maxLen = Math.max(samples.length, dnsSamples.length);
          for (let i = 0; i < maxLen; i++) {
            const ping = samples[i];
            const dns = dnsSamples[i];
            const ts = ping?.timestamp ?? dns?.timestamp;
            ws.addRow([
              ts != null ? localDt(ts) : '',
              ping ? (ping.alive ? 'OK' : 'KO') : '',
              ping?.latency_ms ?? '',
              dns?.latency_ms ?? '',
            ]);
          }
          ws.columns = [24, 10, 12, 12].map(w => ({ width: w }));
        } else {
          ws.addRow([t('monitoring.xlsx_col_time'), t('monitoring.xlsx_col_state'), t('monitoring.xlsx_col_latency')]);
          for (const sample of samples) {
            ws.addRow([localDt(sample.timestamp), sample.alive ? 'OK' : 'KO', sample.latency_ms ?? '']);
          }
          ws.columns = [24, 20, 14].map(w => ({ width: w }));
        }

        // Graphique — toutes les séries (Ping + DNS si présent) dans le même chart
        // Placé col F (col 5) pour ne pas couvrir les données (max col E pour incidents)
        const chartSeries = [{ label, samples }, ...(extraSeries ?? [])];
        const pngB64 = renderChartPng(chartSeries, nowTs);
        const imgId = wb.addImage({ base64: pngB64, extension: 'png' });
        ws.addImage(imgId, { tl: { col: 5, row: 0 } as any, ext: { width: 680, height: 300 } });
      }

      // Téléchargement
      const buffer = await wb.xlsx.writeBuffer();
      const blob = new Blob([buffer], { type: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url; a.download = `lanprobe-sla-${slug}.xlsx`;
      document.body.appendChild(a); a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      showExport = false;
    } finally {
      exporting = false;
    }
  }

  onMount(() => { internetStatus.init(); });

  async function addHost() {
    if (adding) return;
    const ip = newIp.trim();
    if (!ip || $monitoring.has(ip)) return;
    adding = true;
    try {
      monitoring.addHost(ip);
      newIp = '';
      try { await invoke('cmd_start_ping', { ip }); }
      catch (e) { console.error('[Monitoring] cmd_start_ping failed for', ip, e); }
    } finally {
      adding = false;
    }
  }

  async function removeHost(ip: string) {
    await invoke('cmd_stop_ping', { ip });
    monitoring.removeHost(ip);
    if (selectedIp === ip) selectedIp = null;
  }

  interface SlaView {
    ip: string;
    uptime_pct: number;
    avg: number | null;
    min: number | null;
    max: number | null;
    p95: number | null;
    total: number;
    failed: number;
    current_ms: number | null;
    alive: boolean | null;
  }

  function computeSla(host: HostMonitor): SlaView {
    const samples = host.history;
    const total = samples.length;
    const failed = samples.filter(s => !s.alive).length;
    const uptime_pct = total === 0 ? 100 : ((total - failed) / total) * 100;
    const lats = samples
      .filter(s => s.alive && s.latency_ms != null)
      .map(s => s.latency_ms as number)
      .sort((a, b) => a - b);
    const avg = lats.length ? lats.reduce((a, b) => a + b, 0) / lats.length : null;
    const min = lats.length ? lats[0] : null;
    const max = lats.length ? lats[lats.length - 1] : null;
    const p95 = lats.length ? lats[Math.min(lats.length - 1, Math.ceil(lats.length * 0.95) - 1)] : null;
    return {
      ip: host.ip,
      uptime_pct,
      avg, min, max, p95,
      total, failed,
      current_ms: host.current?.latency_ms ?? null,
      alive: host.current?.alive ?? null,
    };
  }

  const hostSlas = $derived([...$monitoring.values()].map(computeSla));

  // Historique local des ticks internet pour calculer avg/p95/min/max —
  // le store ne garde pas la série, il n'expose que le tick courant.
  // `untrack` est CRITIQUE : sans ça l'effet lit + écrit internetHistory
  // et Svelte 5 le détecte comme boucle (effect_update_depth_exceeded),
  // ce qui gelait complètement l'UI sur macOS (navigation impossible).
  let internetHistory = $state<number[]>([]);
  let internetDnsHistory = $state<(number | null)[]>([]);
  $effect(() => {
    const ms = $internetStatus?.icmp_ms;
    const dns = $internetStatus?.dns_ms ?? null;
    if (ms == null) return;
    untrack(() => {
      if (internetHistory.length >= 300) internetHistory.shift();
      internetHistory.push(ms);
      if (internetDnsHistory.length >= 300) internetDnsHistory.shift();
      internetDnsHistory.push(dns);
    });
  });
  const internetStats = $derived.by(() => {
    const lats = [...internetHistory].sort((a, b) => a - b);
    if (lats.length === 0) return { avg: null, p95: null, min: null, max: null } as { avg: number|null; p95: number|null; min: number|null; max: number|null };
    const avg = lats.reduce((a, b) => a + b, 0) / lats.length;
    return {
      avg,
      min: lats[0],
      max: lats[lats.length - 1],
      p95: lats[Math.min(lats.length - 1, Math.ceil(lats.length * 0.95) - 1)],
    };
  });
  // La série internet, mise en forme comme celle d'une cible pour la courbe.
  const internetSeries = $derived(internetHistory.slice(-60).map((ms, i) => ({ timestamp: i, alive: true, latency_ms: ms })));
  const selected = $derived(selectedIp ? $monitoring.get(selectedIp) ?? null : null);
  const selectedSeries = $derived(selectedIp ? Math.max(0, hostSlas.findIndex((h) => h.ip === selectedIp)) % 3 + 1 : 1);
</script>

<div class="screen">
  <ScreenHead title={$_('monitoring.title')}>
    {#snippet acts()}
      {#if !isCell}
        <input class="input m" style="width:220px" bind:value={newIp} placeholder={$_('monitoring.placeholder')}
               aria-label={$_('monitoring.placeholder')} onkeydown={(e) => e.key === 'Enter' && addHost()} />
        <button class="btn primary" type="button" onclick={addHost} disabled={adding}>{$_('monitoring.add')}</button>
      {/if}
      <button class="btn" class:sm={isCell} type="button" onclick={openExport}>{$_('monitoring.export')}</button>
    {/snippet}
  </ScreenHead>

  <div class="screen-body">
    {#if isCell}
      <div class="input-row" style="margin-bottom:8px">
        <input class="input m" style="flex:1;min-width:120px" bind:value={newIp} placeholder={$_('monitoring.placeholder')}
               aria-label={$_('monitoring.placeholder')} onkeydown={(e) => e.key === 'Enter' && addHost()} />
        <button class="btn" type="button" onclick={addHost} disabled={adding}>{$_('monitoring.add')}</button>
      </div>
    {/if}

    <div class="tbl-wrap">
      <table class="t" class:compact={isCell}>
        <thead>
          <tr>
            <th>{$_('monitoring.col_target')}</th>
            <th>{$_('monitoring.col_state')}</th>
            <th>{$_('monitoring.col_recent')}</th>
            <th class="num opt">{$_('monitoring.col_min')}</th>
            <th class="num">{$_('monitoring.col_avg')}</th>
            <th class="num opt">{$_('monitoring.col_p95')}</th>
            <th class="num opt">{$_('monitoring.col_max')}</th>
            <th class="num">{$_('monitoring.col_loss')}</th>
            <th class="num">{$_('monitoring.col_uptime')}</th>
            <th><span class="visually-hidden">{$_('monitoring.col_actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          <!-- Internet : épinglé, toujours en tête. -->
          <tr class:trouble={$internetStatus?.state === 'offline'} class:degr={$internetStatus?.state === 'limited'}>
            <td><span class="nm">{$_('monitoring.internet')}</span><span class="sub2" class:visually-hidden={isCell}>{$_('monitoring.pinned')}</span></td>
            <td><StateMark mark={internetMark($internetStatus?.state)} /></td>
            <td><LatencySpark history={internetSeries} series={2} /></td>
            <td class="num opt">{fmtNum(internetStats.min, lang)}</td>
            <td class="num">{fmtNum(internetStats.avg, lang)}</td>
            <td class="num opt">{fmtNum(internetStats.p95, lang)}</td>
            <td class="num opt">{fmtNum(internetStats.max, lang)}</td>
            <td class="num">{$internetStatus ? fmtNum(100 - $internetStatus.uptime_pct, lang, 1) : '—'}</td>
            <td class="num" title={$_('monitoring.uptime_samples', { values: { n: $internetStatus?.samples ?? 0 } })}>{$internetStatus ? fmtNum($internetStatus.uptime_pct, lang, 2) : '—'}</td>
            <td class="act"></td>
          </tr>
          {#each hostSlas as s, i (s.ip)}
            <tr class="click" class:sel={selectedIp === s.ip} class:trouble={s.alive === false}
                onclick={() => (selectedIp = selectedIp === s.ip ? null : s.ip)}>
              <td>
                <button class="nm m rowbtn" type="button" aria-pressed={selectedIp === s.ip}
                        onclick={(e) => { e.stopPropagation(); selectedIp = selectedIp === s.ip ? null : s.ip; }}>{s.ip}</button>
                <span class="sub2" class:visually-hidden={isCell}>{s.failed > 0
                  ? $_('monitoring.uptime_samples_failed', { values: { n: s.total, f: s.failed } })
                  : $_('monitoring.uptime_samples', { values: { n: s.total } })}</span>
              </td>
              <td><StateMark mark={targetMark(s.alive)} /></td>
              <td><LatencySpark history={$monitoring.get(s.ip)?.history ?? []} series={(i % 3) + 1} /></td>
              <td class="num opt">{fmtNum(s.min, lang)}</td>
              <td class="num">{fmtNum(s.avg, lang)}</td>
              <td class="num opt">{fmtNum(s.p95, lang)}</td>
              <td class="num opt">{fmtNum(s.max, lang)}</td>
              <td class="num">{fmtNum(100 - s.uptime_pct, lang, 1)}</td>
              <td class="num">{fmtNum(s.uptime_pct, lang, 2)}</td>
              <td class="act">
                <button class="btn sm quiet" type="button" onclick={(e) => { e.stopPropagation(); removeHost(s.ip); }}>{$_('monitoring.remove')}</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if hostSlas.length === 0}
        <div class="state"><p>{$_('monitoring.empty')}</p></div>
      {/if}
    </div>

    {#if selected && !isCell}
      <section class="panel" style="margin-top:16px" aria-labelledby="h-mc">
        <div class="chart-head">
          <h2 id="h-mc" class="m">{selected.ip}</h2>
          <span class="muted">{$_('monitoring.chart_sub')}</span>
          <span class="acts"><button class="btn sm quiet" type="button" onclick={() => (selectedIp = null)}>{$_('common.close')}</button></span>
        </div>
        <LatencyChart history={selected.history} series={selectedSeries} />
        <div class="legend" style="margin-top:8px">
          <span><span class="sw s{selectedSeries}"></span>{$_('monitoring.col_latency')}</span>
          <span><span class="sw b" style="width:3px"></span>{$_('monitoring.legend_lost')}</span>
        </div>
      </section>
    {:else if !isCell && hostSlas.length > 0}
      <p class="hint" style="margin-top:8px">{$_('monitoring.select_hint')}</p>
    {/if}
  </div>
</div>

{#if showExport}
  <div class="overlay" role="presentation" onclick={() => showExport = false}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="h-exp" tabindex="-1" onclick={(e) => e.stopPropagation()}>
      <h2 id="h-exp">{$_('monitoring.export_modal_title')}</h2>
      <div class="input-row">
        <button class="btn sm" type="button" onclick={exportSelectAll}>{$_('monitoring.export_select_all')}</button>
        <button class="btn sm" type="button" onclick={exportDeselectAll}>{$_('monitoring.export_deselect_all')}</button>
      </div>
      <div class="export-list">
        <label class="check">
          <input type="checkbox" checked={exportIncludeInternet} onchange={() => exportIncludeInternet = !exportIncludeInternet} />
          <span>{$_('monitoring.internet')}</span>
          <span class="muted">{$_('monitoring.pinned')}</span>
        </label>
        {#each [...$monitoring.keys()] as ip}
          <label class="check">
            <input type="checkbox" checked={exportSelected.has(ip)} onchange={() => toggleExportIp(ip)} />
            <span class="m">{ip}</span>
          </label>
        {/each}
        {#if $monitoring.size === 0}
          <p class="muted">{$_('monitoring.empty')}</p>
        {/if}
      </div>
      {#if !exportIncludeInternet && exportSelected.size === 0}
        <p class="err-text">{$_('monitoring.export_empty')}</p>
      {/if}
      <div class="row">
        <button class="btn" type="button" onclick={() => showExport = false}>{$_('monitoring.export_cancel')}</button>
        <button class="btn primary" type="button"
          disabled={exporting || (!exportIncludeInternet && exportSelected.size === 0)}
          aria-busy={exporting}
          onclick={doExportXlsx}>
          {exporting ? $_('monitoring.export_exporting') : $_('monitoring.export_xlsx')}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .rowbtn { border: 0; background: none; padding: 0; cursor: pointer; text-align: left; }
  .rowbtn:hover { text-decoration: underline; }
  .export-list { display: grid; gap: var(--space-xs); max-height: 260px; overflow-y: auto; }
</style>
