<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import { _, locale } from 'svelte-i18n';
  import { api, ApiError, type MetricsWindow, type Probe } from '$lib/api';
  import { fleet } from '$lib/fleet';
  import { livenessOf } from '$lib/fleet-filter';
  import { absoluteTime, humanBytes, now, elapsed } from '$lib/time';
  import {
    fetchReportFile,
    hubLacksReports,
    parseReports,
    pollReport,
    reportErrorMessage,
    reportRows,
    reportsApi,
    saveReportFile,
    type ReportRow,
  } from '$lib/reports';
  import {
    byPublicIp,
    coverageLabel,
    siteOutages,
    summaryLines,
    type SlaPayload,
    type SummaryLine,
  } from '$lib/sla-report';
  import StateBlock from '$lib/components/StateBlock.svelte';

  /**
   * Rapport SLA d'un site : la période, les sondes, l'aperçu de ce que dira le
   * classeur, puis la demande au hub et l'historique.
   *
   * 🔴 **Le classeur est produit par le HUB** (contrat § 23) : un seul
   * générateur pour un seul document. Le chemin navigateur reste en repli pour
   * un hub plus ancien que la route — on ajoute un chemin, on n'en enlève pas.
   *
   * ⚠️ L'aperçu calcule avec `summaryLines`, la fonction même du classeur de
   * repli : l'écran ne peut pas annoncer un autre chiffre que le document.
   */
  const { site: siteId, onExpired } = $props<{ site: string; onExpired: () => void }>();
  const lang = $derived($locale ?? 'en');
  const t = (k: string, v?: Record<string, string | number>) => $_(k, { values: v });

  const site = $derived($fleet.sites.find((s) => s.site_id === siteId) ?? null);
  const siteName = $derived(site?.name ?? '');
  const candidates = $derived($fleet.probes.filter((p) => p.site_id === siteId));

  // ── Période ──────────────────────────────────────────────────────────────
  type Period = '-24h' | '-7d' | '-30d' | 'month' | 'custom';
  let period = $state<Period>('-7d');
  let from = $state('');
  let to = $state('');

  const pad = (n: number) => String(n).padStart(2, '0');
  const iso = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;

  /**
   * « Mois » : le mois civil PRÉCÉDENT, entier. C'est la période qu'on facture ;
   * le mois en cours donnerait un chiffre qui bouge à chaque ouverture.
   */
  function pick(p: Period) {
    period = p;
    if (p === 'month') {
      const n = new Date();
      from = iso(new Date(n.getFullYear(), n.getMonth() - 1, 1));
      to = iso(new Date(n.getFullYear(), n.getMonth(), 0));
    }
  }

  /** Une période précise n'est exportable que si elle est complète et ordonnée. */
  const ready = $derived(
    (period !== 'custom' && period !== 'month') || (from !== '' && to !== '' && from <= to),
  );

  /**
   * ⚠️ La borne de fin couvre le jour ENTIER : « du 1er au 31 » sans ça
   * s'arrêterait à minuit le 31 au matin.
   */
  function slaWindow(): MetricsWindow {
    return period === 'custom' || period === 'month'
      ? {
          start: Math.floor(new Date(`${from}T00:00:00`).getTime() / 1000),
          stop: Math.floor(new Date(`${to}T23:59:59`).getTime() / 1000),
        }
      : { range: period };
  }

  // ── Sondes ───────────────────────────────────────────────────────────────
  // Toutes cochées d'office : le cas courant est « tout le site ». Mais on
  // peut en retirer une — un rapport qui montre la sonde d'un autre client
  // serait pire qu'inutile.
  let picked = $state(new SvelteSet<string>());
  let pickedInit = false;
  $effect(() => {
    if (pickedInit || candidates.length === 0) return;
    pickedInit = true;
    picked = new SvelteSet(candidates.map((p) => p.probe_id));
    void loadPreview();
  });
  function toggle(id: string) {
    if (picked.has(id)) picked.delete(id);
    else picked.add(id);
  }
  const chosen = $derived(candidates.filter((p) => picked.has(p.probe_id)));

  // ── Aperçu ───────────────────────────────────────────────────────────────
  // ⚠️ Lu à la demande, une sonde après l'autre : trente sondes en parallèle,
  // ce sont trente requêtes Flux simultanées sur le même Influx.
  let preview = $state<SlaPayload[] | null>(null);
  let previewBusy = $state(false);
  let previewError = $state('');
  /** Ce que l'aperçu affiché couvre : il se dit périmé quand les choix changent. */
  let previewKey = $state('');
  const choiceKey = $derived(
    `${period}|${from}|${to}|${chosen.map((p) => p.probe_id).join(',')}`,
  );
  let previewAbort = { aborted: false };

  async function loadPreview() {
    if (!ready || chosen.length === 0) return;
    previewAbort.aborted = true;
    const abort = { aborted: false };
    previewAbort = abort;
    previewBusy = true;
    previewError = '';
    const key = choiceKey;
    const window = slaWindow();
    try {
      const out: SlaPayload[] = [];
      for (const p of chosen) {
        if (abort.aborted) return;
        out.push(await api.sla(p.probe_id, window));
      }
      if (abort.aborted) return;
      preview = out;
      previewKey = key;
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      previewError = e instanceof ApiError ? e.message : String(e);
    } finally {
      if (!abort.aborted) previewBusy = false;
    }
  }

  const lines = $derived<SummaryLine[]>(preview ? summaryLines(preview, t) : []);
  const cuts = $derived(siteOutages(lines));
  const ipRows = $derived(
    (preview ?? []).flatMap((p) =>
      byPublicIp(p.internet, p.public_ip_history).map((r) => ({ ...r, probe: p.probe })),
    ),
  );
  const stale = $derived(preview !== null && previewKey !== choiceKey);

  const pct = $derived(
    new Intl.NumberFormat(lang, { minimumFractionDigits: 2, maximumFractionDigits: 2 }),
  );
  const one = $derived(
    new Intl.NumberFormat(lang, { minimumFractionDigits: 1, maximumFractionDigits: 1 }),
  );
  const short = (s: number) =>
    new Intl.DateTimeFormat(lang, {
      day: '2-digit',
      month: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
    }).format(new Date(s * 1000));

  // ── Demande au hub ───────────────────────────────────────────────────────
  let busy = $state(false);
  let error = $state('');
  /** Où en est le hub : « sonde 2 sur 3 — Lyon », puis le téléchargement. */
  let step = $state('');
  /** Le classeur vient du navigateur, pas du hub. Se dit, ne se devine pas. */
  let fallback = $state(false);
  /** Écran quitté : le suivi cesse d'interroger le hub. */
  let abort = { aborted: false };

  async function run() {
    busy = true;
    error = '';
    fallback = false;
    step = $_('report.hub_preparing');
    abort = { aborted: false };
    const window = slaWindow();
    const list = chosen;
    try {
      const { report_id } = await reportsApi.requestReport(siteId, {
        // ⚠️ La liste part TOUJOURS explicite : « absent » voudrait dire
        // « toutes les sondes du site ».
        probe_ids: list.map((p) => p.probe_id),
        window,
      });
      const record = await pollReport(report_id, {
        signal: abort,
        // Le hub ne prépare qu'un rapport à la fois pour tout le parc :
        // l'attente peut durer, et un bouton muet se lit comme une panne.
        onStep: (r) => (step = r.step ?? $_('report.hub_preparing')),
      });
      await loadHistory();
      if (record.state !== 'ready') {
        error = $_('report.hub_failed', { values: { cause: record.error ?? '—' } });
        return;
      }
      step = $_('report.hub_download');
      saveReportFile(await fetchReportFile(record.report_id, record.file_name ?? ''));
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      if (hubLacksReports(e)) return void (await inBrowser(list, window));
      error = reportErrorMessage(e, t, lang);
    } finally {
      busy = false;
      step = '';
    }
  }

  /** Le générateur du navigateur, en repli — séquentiel, comme le hub. */
  async function inBrowser(list: Probe[], window: MetricsWindow) {
    try {
      const payloads = [];
      for (const p of list) payloads.push(await api.sla(p.probe_id, window));
      const { downloadSlaWorkbook } = await import('$lib/sla-report');
      await downloadSlaWorkbook(payloads, siteName, t, lang);
      fallback = true;
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      error = e instanceof ApiError ? e.message : String(e);
    }
  }

  // ── Historique ───────────────────────────────────────────────────────────
  // 🔴 Une ligne d'historique n'est pas un fichier, c'est une trace : qui a
  // extrait les données de ce client, quand, sur quelle fenêtre. Le fichier
  // part à la purge ; la ligne, jamais.
  let history = $state<ReportRow[]>([]);
  let historyError = $state('');
  let redownloading = $state('');

  async function loadHistory() {
    historyError = '';
    try {
      history = reportRows(parseReports(await reportsApi.listSiteReports(siteId)));
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      // Un hub plus ancien n'a pas d'historique : ce n'est pas une panne.
      history = [];
      if (!hubLacksReports(e)) historyError = e instanceof ApiError ? e.message : String(e);
    }
  }

  /** Mêmes contrôles qu'au premier téléchargement : longueur et empreinte. */
  async function redownload(reportId: string, fileName: string) {
    redownloading = reportId;
    error = '';
    try {
      saveReportFile(await fetchReportFile(reportId, fileName));
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      error = reportErrorMessage(e, t, lang);
      await loadHistory();
    } finally {
      redownloading = '';
    }
  }

  onMount(() => {
    if ($fleet.sites.length === 0) void fleet.load(onExpired);
    void loadHistory();
  });
  onDestroy(() => {
    // ⚠️ Le rapport, lui, continue d'être produit : il figure dans
    // l'historique, prêt à télécharger. Rien n'est annulé.
    abort.aborted = true;
    previewAbort.aborted = true;
  });

  const PERIODS: Period[] = ['-24h', '-7d', '-30d', 'month', 'custom'];
</script>

<nav class="lp-crumbs" aria-label={$_('nav.fleet')}>
  <a href="#/">{$_('nav.fleet')}</a><span class="sep">/</span>
  <span>{siteName}</span>
</nav>
<header class="lp-head">
  <div>
    <h1>{$_('report.title')}</h1>
    {#if siteName}<p class="lp-sub">{siteName}</p>{/if}
  </div>
  <div class="lp-acts">
    <button class="lp-btn primary" onclick={run} disabled={busy || chosen.length === 0 || !ready}>
      {busy ? $_('sla.exporting') : $_('report.download_workbook')}
    </button>
  </div>
</header>

{#if $fleet.loading && !site}
  <StateBlock tone="loading" title={$_('common.loading')} />
{:else if !site}
  <StateBlock title={$_('report.no_site')} />
{:else}
  <!-- ⚠️ L'avancement du hub, en clair : il ne prépare qu'un rapport à la
       fois pour tout le parc. -->
  {#if busy && step}<p class="lp-banner info" role="status">{step}</p>{/if}
  {#if fallback}<p class="lp-banner info">{$_('report.hub_fallback')}</p>{/if}
  {#if error}<p class="lp-banner bad" role="alert">{error}</p>{/if}

  <section class="lp-card controls">
    <div class="row">
      <span class="lbl">{$_('report.period')}</span>
      <div class="lp-seg" role="group" aria-label={$_('report.period')}>
        {#each PERIODS as p (p)}
          <button aria-pressed={period === p} onclick={() => pick(p)}>{$_(`report.period_${p.replace('-', '')}`)}</button>
        {/each}
      </div>
      {#if period === 'custom' || period === 'month'}
        <!-- ⚠️ Pour tout rapport contractuel, ce sont des dates qu'il faut :
             une fenêtre glissante donne un chiffre différent à chaque
             ouverture du document. -->
        <label class="dates">
          <span class="lp-sr">{$_('sla.from')}</span>
          <input class="lp-input" type="date" bind:value={from} max={to || undefined} onchange={() => (period = 'custom')} />
        </label>
        <span class="muted">{$_('sla.to')}</span>
        <label class="dates">
          <span class="lp-sr">{$_('sla.to')}</span>
          <input class="lp-input" type="date" bind:value={to} min={from || undefined} onchange={() => (period = 'custom')} />
        </label>
      {/if}
    </div>
    <div class="row">
      <span class="lbl">{$_('report.probes')}</span>
      <div class="picks">
        {#each candidates as p (p.probe_id)}
          <label class="pick">
            <input type="checkbox" checked={picked.has(p.probe_id)} onchange={() => toggle(p.probe_id)} />
            <span>{p.name}</span>
            <!-- Même verdict que la ligne du parc. -->
            <span class="muted">{$_(`status.${livenessOf(p, $now)}`)}</span>
          </label>
        {/each}
      </div>
    </div>
  </section>

  <div class="lp-sec-head">
    <h2>{$_('report.preview')}</h2>
    <div class="lp-acts">
      {#if stale}<span class="muted">{$_('report.preview_stale')}</span>{/if}
      <button class="lp-btn ghost sm" onclick={loadPreview} disabled={previewBusy || !ready || chosen.length === 0}>
        {previewBusy ? $_('common.loading') : $_('report.preview_refresh')}
      </button>
    </div>
  </div>

  {#if previewError}
    <p class="lp-banner bad" role="alert">{previewError}</p>
  {:else if !preview}
    <p class="muted">{previewBusy ? $_('common.loading') : $_('report.preview_none')}</p>
  {:else}
    <div class="lp-tbl-wrap" class:dim={stale || previewBusy}>
      <table class="lp-t">
        <thead>
          <tr>
            <th>{$_('sla.probe')}</th>
            <th>{$_('sla.col_target')}</th>
            <th class="num">{$_('sla.col_uptime')}</th>
            <th class="num">{$_('sla.col_coverage')}</th>
            <th class="num">{$_('report.col_avg')}</th>
            <th class="num">{$_('report.col_p95')}</th>
            <th class="num">{$_('sla.col_lost')}</th>
            <th class="num">{$_('sla.col_outages')}</th>
          </tr>
        </thead>
        <tbody>
          {#each lines as l, i (i)}
            <tr>
              <td>{l.probe}</td>
              <td>{l.label}</td>
              <td class="num">
                {#if l.stats.uptime_pct != null}{pct.format(l.stats.uptime_pct)}<span class="lp-u">%</span>{:else}<span class="muted">{$_('sla.undetermined')}</span>{/if}
              </td>
              <!-- ⚠️ Sans couverture annoncée, on ne sait pas : « — », jamais un
                   100 % inventé. -->
              <td class="num">{l.coverage ? (coverageLabel(l.coverage, t) ?? '100 %') : '—'}</td>
              <td class="num">{l.stats.avg != null ? one.format(l.stats.avg) : '—'}</td>
              <td class="num">{l.stats.p95 != null ? one.format(l.stats.p95) : '—'}</td>
              <td class="num">
                {#if l.stats.total > 0}{one.format((l.stats.failed / l.stats.total) * 100)}<span class="lp-u">%</span>{:else}—{/if}
              </td>
              <td class="num">{l.outages.length}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <div class="lp-sec-head">
      <h2>{$_('sla.sheet_outages')} <span class="lp-sub">{$_('report.outages_count', { values: { n: cuts.length } })}</span></h2>
    </div>
    {#if cuts.length === 0}
      <p class="muted">{$_('sla.no_outage')}</p>
    {:else}
      <div class="lp-tbl-wrap" class:dim={stale || previewBusy}>
        <table class="lp-t">
          <thead>
            <tr>
              <th>{$_('sla.probe')}</th>
              <th>{$_('sla.col_target')}</th>
              <th class="num">{$_('sla.col_start')}</th>
              <th class="num">{$_('sla.col_end')}</th>
              <th class="num">{$_('sla.col_duration')}</th>
            </tr>
          </thead>
          <tbody>
            {#each cuts.slice(0, 50) as o, i (i)}
              <tr class:open={o.end == null}>
                <td>{o.probe}</td>
                <td>{o.label}</td>
                <td class="num">{short(o.start)}</td>
                <td class="num">{o.end == null ? $_('report.ongoing') : short(o.end)}</td>
                <td class="num">{elapsed(Math.max(1, (o.end ?? $now / 1000) - o.start), lang)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if cuts.length > 50}<p class="muted">{$_('report.outages_more', { values: { n: cuts.length - 50 } })}</p>{/if}
    {/if}

    {#if ipRows.length > 0}
      <div class="lp-sec-head"><h2>{$_('report.ip_title')}</h2></div>
      <div class="lp-tbl-wrap" class:dim={stale || previewBusy}>
        <table class="lp-t">
          <thead>
            <tr>
              <th>{$_('sla.probe')}</th>
              <th>{$_('report.col_label')}</th>
              <th>{$_('report.col_public_ip')}</th>
              <th>{$_('report.col_gateway')}</th>
              <th class="num">{$_('sla.col_start')}</th>
              <th class="num">{$_('sla.col_end')}</th>
              <th class="num">{$_('sla.col_uptime')}</th>
            </tr>
          </thead>
          <tbody>
            {#each ipRows as r, i (i)}
              <tr>
                <td>{r.probe}</td>
                <td>{r.label ?? ''}</td>
                <td><span class="lp-mono">{r.public_ip ?? '—'}</span></td>
                <td><span class="lp-mono">{r.gateway ?? ''}</span></td>
                <td class="num">{r.from != null ? short(r.from) : ''}</td>
                <td class="num">{r.to != null ? short(r.to) : ''}</td>
                <td class="num">
                  {#if r.uptime_pct != null}{pct.format(r.uptime_pct)}<span class="lp-u">%</span>{:else}—{/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}

  <!-- 🔴 L'historique : qui a extrait les données de ce client, quand, sur
       quelle fenêtre et quelles sondes. Une ligne sans bouton DIT pourquoi. -->
  <div class="lp-sec-head"><h2>{$_('report.hub_history')}</h2></div>
  {#if historyError}
    <p class="lp-banner bad" role="alert">{historyError}</p>
  {:else if history.length === 0}
    <p class="muted">{$_('report.hub_empty')}</p>
  {:else}
    <div class="lp-tbl-wrap">
      <table class="lp-t">
        <thead>
          <tr>
            <th>{$_('report.col_requested')}</th>
            <th>{$_('report.col_period')}</th>
            <th>{$_('report.col_by')}</th>
            <th class="num">{$_('report.col_size')}</th>
            <th class="act"><span class="lp-sr">{$_('accounts.col_actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          {#each history as row (row.report.report_id)}
            <tr>
              <td>{absoluteTime(row.report.requested_at, lang)}</td>
              <td>
                {short(row.report.range_start)} → {short(row.report.range_stop)}
                <span class="sub2">{$_('report.hub_probes', { values: { n: row.report.probe_ids.length } })}</span>
              </td>
              <!-- « depuis le hub web » quand aucun appareil n'est en cause. -->
              <td>
                {row.report.requested_by}
                {#if row.report.device_id == null}<span class="sub2">{$_('report.hub_requested_from_web')}</span>{/if}
              </td>
              <td class="num">{row.downloadable ? humanBytes(row.report.file_size, lang) : ''}</td>
              <td class="act state">
                {#if row.downloadable}
                  <button
                    class="lp-btn ghost sm"
                    disabled={redownloading !== ''}
                    title={row.report.expires_at != null
                      ? $_('report.hub_expires_at', { values: { date: absoluteTime(row.report.expires_at, lang) } })
                      : undefined}
                    onclick={() => redownload(row.report.report_id, row.report.file_name ?? '')}
                  >
                    {redownloading === row.report.report_id ? $_('sla.exporting') : $_('report.hub_download')}
                  </button>
                {:else if row.purged}
                  <!-- ⚠️ « Fichier purgé le … », jamais un bouton qui échoue
                       en silence : le bon geste est de le redemander. -->
                  <span class="muted">{$_('report.hub_purged_hint', { values: { date: absoluteTime(row.report.purged_at, lang) } })}</span>
                {:else if row.failed}
                  <span class="bad">{$_('report.hub_failed', { values: { cause: row.report.error ?? '—' } })}</span>
                {:else if row.preparing}
                  <span class="muted">{row.report.step ?? $_('report.hub_preparing')}</span>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
{/if}

<style>
  .controls {
    padding: 16px;
    display: grid;
    gap: 16px;
    margin-bottom: 8px;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 16px;
  }
  .lbl {
    width: 80px;
    font-size: 0.875rem;
    color: var(--color-ink-2);
  }
  .dates input {
    width: 160px;
  }
  .picks {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 24px;
  }
  .pick {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 0.875rem;
    cursor: pointer;
  }
  .muted {
    color: var(--color-muted);
    font-size: 0.8125rem;
  }
  .lp-sec-head h2 .lp-sub {
    font-weight: 400;
    margin-left: 8px;
  }
  .dim {
    opacity: 0.55;
    transition: opacity 0.15s;
  }
  tr.open td:first-child {
    box-shadow: inset 2px 0 0 var(--st-bad);
  }
  td.act.state {
    white-space: normal;
    max-width: 360px;
    text-align: right;
  }
  .bad {
    color: var(--st-bad);
    font-size: 0.8125rem;
  }
</style>
