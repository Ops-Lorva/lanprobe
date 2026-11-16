<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { newestVersion } from '$lib/probe-version';
  import { canOperate } from '$lib/session';
  import { SvelteSet } from 'svelte/reactivity';
  import { RANGES, type Range } from '$lib/metrics';
  import { _, locale } from 'svelte-i18n';
  import { fleet } from '$lib/fleet';
  import { flash } from '$lib/flash';
  import { absoluteTime, humanBytes, now, relativeTime } from '$lib/time';
  import { api, ApiError, type MetricsWindow, type Probe, type Site } from '$lib/api';
  import { LANGS, type Lang } from '$lib/i18n';
  // Le classeur vient du hub (contrat § 23). `sla-report` reste le repli.
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
  import type { LivenessState } from '$lib/probe-liveness';
  // ⚠️ Filtres, compteurs et tris passent TOUS par ce module, jamais par
  // `p.status` : le hub tolère trois battements manqués, et une sonde affichée
  // « Silencieuse » sur sa ligne se retrouvait comptée « En ligne » dans le
  // bandeau et retenue par le filtre « En ligne ».
  import {
    livenessOf,
    matchesFleetFilter,
    needsAttention,
    siteSeverity,
    sortKey,
    type FleetFilter,
  } from '$lib/fleet-filter';
  import FleetRail from '$lib/components/FleetRail.svelte';
  import SiteSection from '$lib/components/SiteSection.svelte';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import SiteAlerts from '$lib/components/SiteAlerts.svelte';
  import { fleetBeats } from '$lib/fleet-beats';
  import { displayTotals, probeDisplay } from '$lib/probe-state';
  import { contactTime } from '$lib/time';

  const { onExpired } = $props<{ onExpired: () => void }>();

  const lang = $derived($locale ?? 'en');

  // ── Filtres (une seule barre, au-dessus de tout ce qu'elle cadre) ─────────
  let search = $state('');
  let statusFilter = $state<FleetFilter>('all');
  let sort = $state<'status' | 'name' | 'last_seen'>('status');

  const filtered = $derived.by(() => {
    const q = search.trim().toLowerCase();
    return $fleet.probes.filter((p) => {
      if (!matchesFleetFilter(p, statusFilter, $now)) return false;
      if (!q) return true;
      return [p.name, p.site, p.platform, p.version]
        .filter(Boolean)
        .some((f) => String(f).toLowerCase().includes(q));
    });
  });

  function sortProbes(list: Probe[]) {
    const copy = [...list];
    if (sort === 'name') copy.sort((a, b) => a.name.localeCompare(b.name, lang));
    else if (sort === 'last_seen') copy.sort((a, b) => (b.last_seen ?? 0) - (a.last_seen ?? 0));
    else
      copy.sort(
        (a, b) =>
          sortKey(a, $now) - sortKey(b, $now) ||
          b.buffered_points - a.buffered_points ||
          a.name.localeCompare(b.name, lang),
      );
    return copy;
  }

  /** Regroupement par site, en partant de la liste des sites : un site sans
      sonde doit apparaître, sinon on ne peut pas lui en ajouter une. */
  const groups = $derived.by(() => {
    const byId = new Map<string, Probe[]>();
    for (const p of filtered) {
      const list = byId.get(p.site_id) ?? [];
      list.push(p);
      byId.set(p.site_id, list);
    }
    const visible = $fleet.siteId
      ? $fleet.sites.filter((s) => s.site_id === $fleet.siteId)
      : $fleet.sites;

    const out = visible.map((s) => ({
      site: s,
      probes: sortProbes(byId.get(s.site_id) ?? []),
    }));

    // Un site absent de /api/sites mais porteur de sondes ne doit pas
    // disparaître de l'écran : on le rattrape depuis les sondes elles-mêmes.
    for (const [siteId, list] of byId) {
      if (!out.some((g) => g.site.site_id === siteId)) {
        out.push({
          site: { site_id: siteId, name: list[0].site, probe_count: list.length },
          probes: sortProbes(list),
        });
      }
    }

    out.sort((a, b) =>
      sort === 'status'
        ? siteSeverity(a.probes, $now) - siteSeverity(b.probes, $now) ||
          a.site.name.localeCompare(b.site.name, lang)
        : a.site.name.localeCompare(b.site.name, lang),
    );
    return out;
  });

  // ⚠️ `$now` et non `$fleet.loadedAt` : l'horloge partagée vieillit l'écran
  // toutes les 15 s, et c'est elle qui fait glisser une sonde d'« en ligne »
  // vers « silencieuse » entre deux chargements du parc. Sans elle, le bandeau
  // resterait figé sur le verdict du dernier rafraîchissement.
  const totals = $derived(displayTotals($fleet.probes, $now));

  /** Les sondes qui ne battent plus, pour le bandeau : c'est elles qu'on vient voir. */
  const down = $derived(
    $fleet.probes.filter((p) => probeDisplay(p, $now).state === 'offline'),
  );
  /**
   * La référence de « pas à jour » : la dernière version PUBLIÉE quand le hub
   * arrive à la lire, la plus récente du parc sinon.
   *
   * 🔴 Les deux répondent à des questions différentes, et la première est la
   * bonne : avec le parc seul, un parc entièrement en retard ne signale rien.
   * ⚠️ Mais le repli n'est pas un pis-aller : un hub sans sortie internet
   * fonctionne parfaitement — ce sont les sondes qui l'appellent — et
   * « laquelle traîne derrière les autres » reste utile.
   */
  let publishedVersion = $state<string | null>(null);
  $effect(() => {
    // Un échec ne dit rien à l'écran : l'absence de couleur n'est pas une
    // alerte, et un bandeau « GitHub injoignable » devant un parc serait du
    // bruit pour un chiffre accessoire.
    api.latestProbeRelease().then((r) => (publishedVersion = r.latest)).catch(() => {});
  });
  const newestVersionSeen = $derived(
    publishedVersion ?? newestVersion($fleet.probes.map((p) => p.version)),
  );

  const bufferingCount = $derived($fleet.probes.filter((p) => p.buffered_points > 0).length);
  const hasFilter = $derived(search.trim() !== '' || statusFilter !== 'all');

  // ── Dépliage ─────────────────────────────────────────────────────────────
  let manual = $state<Record<string, boolean>>({});

  function isExpanded(siteId: string, probes: Probe[], siteCount: number) {
    if (siteId in manual) return manual[siteId];
    // Peu de sites : tout ouvert, on voit le parc d'un bloc.
    // Beaucoup de sites : seuls ceux qui demandent une action s'ouvrent, les
    // autres restent pliés — mais leur en-tête dit déjà comment ils vont.
    if (siteCount <= 3) return true;
    return needsAttention(probes, $now);
  }

  // ── Enrôlement ────────────────────────────────────────────────────────────
  // « Ajouter une sonde » et le « + » de chaque site mènent à `#/add` : le
  // code, l'adresse du hub et l'installeur tiennent sur un seul écran. Les
  // codes en attente y sont listés (ils étaient ici, dans la liste du site).

  // ── Sites : création / renommage ─────────────────────────────────────────
  let createOpen = $state(false);
  let createName = $state('');
  let createBusy = $state(false);
  let createError = $state('');

  async function submitCreate() {
    const name = createName.trim();
    if (!name) return void (createError = $_('site.empty_name'));
    createBusy = true;
    createError = '';
    try {
      await api.createSite(name);
      createOpen = false;
      createName = '';
      await fleet.load(onExpired, { quiet: true });
    } catch (e) {
      createError =
        e instanceof ApiError
          ? e.isConflict
            ? $_('site.conflict', { values: { name } })
            : e.message
          : String(e);
    } finally {
      createBusy = false;
    }
  }

  let renameTarget = $state<{ id: string; name: string } | null>(null);
  let renameName = $state('');
  /**
   * 🔴 **La langue des rapports de ce client**, réglée là où on nomme le site
   * (contrat § 23). `''` = défaut du hub, pas une absence de choix impossible
   * à exprimer : un site doit pouvoir revenir au défaut.
   *
   * ⚠️ Ce n'est PAS la langue de l'interface — celle-là est dans la barre et
   * suit le compte. Les confondre laissait le même client recevoir deux
   * rapports dans deux langues selon qui avait cliqué.
   */
  let renameLocale = $state<Lang | ''>('');
  let renameBusy = $state(false);
  let renameError = $state('');

  function openRename(site: Site) {
    renameTarget = { id: site.site_id, name: site.name };
    renameName = site.name;
    renameLocale = site.report_locale ?? '';
    renameError = '';
  }

  async function submitRename() {
    if (!renameTarget) return;
    const name = renameName.trim();
    if (!name) return void (renameError = $_('site.empty_name'));
    renameBusy = true;
    renameError = '';
    try {
      // Le champ est TOUJOURS envoyé ici : l'écran vient de montrer sa valeur
      // courante, et `null` veut dire « défaut du hub ». Les autres appelants
      // l'omettent pour ne pas y toucher.
      await api.renameSite(renameTarget.id, name, renameLocale === '' ? null : renameLocale);
      fleet.renameSiteLocal(renameTarget.id, name);
      renameTarget = null;
      await fleet.load(onExpired, { quiet: true });
    } catch (e) {
      renameError =
        e instanceof ApiError
          ? e.isConflict
            ? $_('site.conflict', { values: { name } })
            : e.message
          : String(e);
    } finally {
      renameBusy = false;
    }
  }

  // ── Rapport SLA d'un site ────────────────────────────────────────────────
  //
  // ⚠️ Le rapport se demande depuis le SITE, parce que c'est l'unité qu'on
  // remet à un client. On choisit les sondes à y faire figurer : toutes ne le
  // concernent pas forcément, et un rapport qui expose une sonde d'un autre
  // client serait pire qu'inutile.
  // ── Archivage ─────────────────────────────────────────────────────────────
  //
  // 🔴 Archiver, pas supprimer. Un client qu'on ne sert plus doit sortir du
  // parc, mais effacer son site rendrait ses mesures orphelines : son rapport
  // SLA de l'année écoulée n'aurait plus de site auquel se rattacher. Tout
  // reste, et l'opération se défait.
  let archiveTarget = $state<{ id: string; name: string; archived: boolean } | null>(null);
  let archiveBusy = $state(false);
  let archiveError = $state('');

  const archivedCount = $derived($fleet.sites.filter((s) => s.archived_at).length);

  async function submitArchive() {
    if (!archiveTarget) return;
    archiveBusy = true;
    archiveError = '';
    try {
      await api.archiveSite(archiveTarget.id, archiveTarget.archived);
      archiveTarget = null;
      await fleet.load(onExpired, { quiet: true });
    } catch (e) {
      archiveError = e instanceof ApiError ? e.message : String(e);
    } finally {
      archiveBusy = false;
    }
  }

  // ── Alertes du site ───────────────────────────────────────────────────────
  //
  // Le panneau se monte à l'ouverture et relit ses abonnements à chaque
  // écriture : c'est le hub qui résout l'héritage site → sonde, et le
  // recalculer ici en serait une deuxième implémentation.
  let alertsSiteId = $state('');
  let alertsSiteName = $state('');

  function openAlerts(siteId: string, siteName: string) {
    alertsSiteId = siteId;
    alertsSiteName = siteName;
  }

  let slaSiteId = $state('');
  let slaSiteName = $state('');
  let slaPicked = $state(new SvelteSet<string>());
  let slaRange = $state<Range | 'custom'>('-7d');
  // Bornes explicites, au format `YYYY-MM-DD` des champs date.
  let slaFrom = $state('');
  let slaTo = $state('');
  let slaBusy = $state(false);
  let slaError = $state('');
  /** Où en est le hub : « sonde 2 sur 3 — Lyon », puis le téléchargement. */
  let slaStep = $state('');
  /** Le classeur vient du navigateur, pas du hub. Se dit, ne se devine pas. */
  let slaFallback = $state(false);
  /** Modal fermé : le suivi cesse d'interroger le hub. */
  let slaAbort = { aborted: false };

  /** Une période précise n'est exportable que si elle est complète et ordonnée. */
  const slaReady = $derived(
    slaRange !== 'custom' || (slaFrom !== '' && slaTo !== '' && slaFrom <= slaTo),
  );

  const slaCandidates = $derived(
    $fleet.probes.filter((p) => p.site_id === slaSiteId),
  );

  function openSlaExport(siteId: string, siteName: string) {
    slaSiteId = siteId;
    slaSiteName = siteName;
    slaError = '';
    slaFallback = false;
    slaHistory = [];
    void loadSlaHistory();
    // Toutes cochées par défaut : le cas courant est « tout le site », et
    // décocher est plus rapide que cocher trente lignes.
    slaPicked = new SvelteSet(
      $fleet.probes.filter((p) => p.site_id === siteId).map((p) => p.probe_id),
    );
  }

  /**
   * Ferme la fenêtre et cesse d'interroger le hub.
   *
   * ⚠️ Le rapport, lui, continue d'être produit : il figure dans l'historique
   * du site, prêt à télécharger. Rien n'est perdu, et rien n'est annulé — aucune
   * route ne supprime un rapport.
   */
  function closeSlaExport() {
    slaAbort.aborted = true;
    slaSiteId = '';
  }

  function togglePicked(probeId: string) {
    if (slaPicked.has(probeId)) slaPicked.delete(probeId);
    else slaPicked.add(probeId);
  }

  /**
   * La fenêtre du rapport, résolue en bornes exactes.
   *
   * ⚠️ La borne de fin couvre le jour ENTIER : « du 1er au 31 » sans ça
   * s'arrêterait à minuit le 31 au matin, et le dernier jour manquerait au
   * rapport sans que personne ne s'en aperçoive.
   */
  function slaWindow(): MetricsWindow {
    return slaRange === 'custom'
      ? {
          start: Math.floor(new Date(`${slaFrom}T00:00:00`).getTime() / 1000),
          stop: Math.floor(new Date(`${slaTo}T23:59:59`).getTime() / 1000),
        }
      : { range: slaRange };
  }

  /**
   * 🔴 **Le classeur est produit par le HUB** (contrat § 23) : un seul
   * générateur pour un seul document. Le chemin navigateur reste en repli pour
   * un hub plus ancien que la route — on ajoute un chemin, on n'en enlève pas.
   */
  async function runSlaExport() {
    slaBusy = true;
    slaError = '';
    slaFallback = false;
    slaStep = $_('report.hub_preparing');
    slaAbort = { aborted: false };
    const window = slaWindow();
    const chosen = slaCandidates.filter((p) => slaPicked.has(p.probe_id));
    try {
      const { report_id } = await reportsApi.requestReport(slaSiteId, {
        // ⚠️ La liste part TOUJOURS explicite : « absent » voudrait dire
        // « toutes les sondes du site », et une case décochée passerait quand
        // même dans le classeur remis au client.
        probe_ids: chosen.map((p) => p.probe_id),
        window,
      });
      const record = await pollReport(report_id, {
        signal: slaAbort,
        // Le hub ne prépare qu'un rapport à la fois pour tout le parc :
        // l'attente peut durer, et un bouton muet se lit comme une panne.
        onStep: (r) => (slaStep = r.step ?? $_('report.hub_preparing')),
      });
      await loadSlaHistory();
      if (record.state !== 'ready') {
        slaError = $_('report.hub_failed', { values: { cause: record.error ?? '—' } });
        return;
      }
      slaStep = $_('report.hub_download');
      saveReportFile(await fetchReportFile(record.report_id, record.file_name ?? ''));
      slaSiteId = '';
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      if (hubLacksReports(e)) return void (await exportSlaInBrowser(chosen, window));
      slaError = reportErrorMessage(e, (k, v) => $_(k, { values: v }), lang);
    } finally {
      slaBusy = false;
      slaStep = '';
    }
  }

  /**
   * Le générateur du navigateur, en repli.
   *
   * ⚠️ Séquentiel : trente sondes en parallèle, ce sont trente requêtes Flux
   * simultanées sur le même Influx, et un hub qui ne répond plus pendant qu'on
   * produit un rapport. C'est aussi ce que le hub fait de son côté.
   */
  async function exportSlaInBrowser(chosen: Probe[], window: MetricsWindow) {
    try {
      const payloads = [];
      for (const p of chosen) {
        payloads.push(await api.sla(p.probe_id, window));
      }
      const { downloadSlaWorkbook } = await import('$lib/sla-report');
      await downloadSlaWorkbook(payloads, slaSiteName, (k, v) => $_(k, { values: v }), lang);
      slaFallback = true;
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      slaError = e instanceof ApiError ? e.message : String(e);
    }
  }

  // ── Historique des rapports du site ──────────────────────────────────────
  //
  // 🔴 **Une ligne d'historique n'est pas un fichier, c'est une trace** : qui a
  // extrait les données de ce client, quand, sur quelle fenêtre, quelles
  // sondes. Le fichier part à la purge ; la ligne, jamais. C'est pourquoi une
  // ligne sans bouton de téléchargement doit DIRE « fichier purgé le … » —
  // sinon elle se lit comme une panne.
  let slaHistory = $state<ReportRow[]>([]);
  let slaHistoryError = $state('');
  /** Le rapport qu'on retélécharge — un seul à la fois, le bouton le montre. */
  let slaRedownloading = $state('');

  async function loadSlaHistory() {
    slaHistoryError = '';
    try {
      slaHistory = reportRows(parseReports(await reportsApi.listSiteReports(slaSiteId)));
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      // Un hub plus ancien n'a pas d'historique : ce n'est pas une panne, et la
      // liste reste vide sans message rouge.
      slaHistory = [];
      if (!hubLacksReports(e)) {
        slaHistoryError = e instanceof ApiError ? e.message : String(e);
      }
    }
  }

  /**
   * Retélécharge un rapport déjà produit.
   *
   * ⚠️ Le retéléchargement subit EXACTEMENT les mêmes contrôles que le premier :
   * longueur annoncée et empreinte. Un fichier purgé entre-temps rend `410`, et
   * l'écran dit la date plutôt qu'une erreur générique.
   */
  async function redownloadReport(reportId: string, fileName: string) {
    slaRedownloading = reportId;
    slaError = '';
    try {
      saveReportFile(await fetchReportFile(reportId, fileName));
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      slaError = reportErrorMessage(e, (k, v) => $_(k, { values: v }), lang);
      // Le hub vient peut-être de purger : la ligne doit refléter son état.
      await loadSlaHistory();
    } finally {
      slaRedownloading = '';
    }
  }

  // ── Bande de battement ───────────────────────────────────────────────────
  // Lue à part et lentement (`fleet-beats`) : elle compte des quarts d'heure.
  // Le store ne relit que ce qui a plus de 5 min ; on le relance donc à chaque
  // relecture du parc sans craindre l'avalanche.
  $effect(() => {
    const ids = $fleet.probes.map((p) => p.probe_id);
    if (ids.length) void fleetBeats.refresh(ids, onExpired);
  });

  // ── Cycle de vie ─────────────────────────────────────────────────────────
  let timer: ReturnType<typeof setInterval> | null = null;

  onMount(() => {
    void fleet.load(onExpired);
    // Le statut est dérivé du dernier battement : sans rafraîchissement, un
    // parc laissé ouvert affiche un « en ligne » qui a vieilli d'une heure.
    // Le même tour recharge les attentes : c'est ainsi qu'une place réservée
    // disparaît d'elle-même quand la machine a consommé son code.
    timer = setInterval(() => {
      void fleet.load(onExpired, { quiet: true });
    }, 30_000);
  });
  onDestroy(() => {
    if (timer) clearInterval(timer);
  });
</script>

{#if $flash}
  <!-- Confirmation rapportée depuis le détail : elle redit que les mesures
       sont conservées, au moment où l'utilisateur constate la ligne en moins. -->
  <div class="lp-banner info" role="status">
    <span class="grow">{$flash}</span>
    <button class="lp-btn ghost sm" onclick={() => flash.set('')}>{$_('common.close')}</button>
  </div>
{/if}

<header class="lp-head">
  <h1>{$_('fleet.title')}</h1>
  <div class="lp-acts">
    <!-- Un seul bouton plein : l'action de l'écran. Les autres restent sobres. -->
    {#if $canOperate}
      <button class="lp-btn ghost" onclick={() => (createOpen = true)}>{$_('fleet.new_site')}</button>
    {/if}
    <!-- ⚠️ Le chemin vers les archives ne disparaît jamais, même à zéro
         archive : sans lui, on archive un site puis on cherche comment le
         faire revenir. -->
    <button
      class="lp-btn ghost"
      onclick={() => fleet.toggleArchived(onExpired)}
      aria-pressed={$fleet.showArchived}
    >
      {$fleet.showArchived
        ? $_('fleet.hide_archived')
        : archivedCount > 0
          ? $_('fleet.show_archived_n', { values: { n: archivedCount } })
          : $_('fleet.show_archived')}
    </button>
    {#if $canOperate}
      <a class="lp-btn primary" href="#/add">
        <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><path d="M8 3v10M3 8h10" /></svg>
        {$_('fleet.add_probe')}
      </a>
    {/if}
  </div>
</header>

<FleetRail
  sites={$fleet.sites.length}
  total={$fleet.probes.length}
  {totals}
  filter={statusFilter}
  onpick={(f) => (statusFilter = statusFilter === f ? 'all' : f)}
/>

{#if down.length === 1}
  <div class="lp-banner bad" role="status">
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><rect x="1" y="1" width="8" height="8" rx="1" fill="var(--st-bad)" /></svg>
    <span class="grow">
      {$_('fleet.down_one', {
        values: {
          name: down[0].name,
          site: down[0].site,
          when: contactTime(down[0].last_seen, lang, $now),
        },
      })}
    </span>
    <a class="lp-btn sm" href="#/probes/{encodeURIComponent(down[0].probe_id)}">{$_('fleet.down_open')}</a>
  </div>
{:else if down.length > 1}
  <div class="lp-banner bad" role="status">
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><rect x="1" y="1" width="8" height="8" rx="1" fill="var(--st-bad)" /></svg>
    <span class="grow">{$_('fleet.down_many', { values: { n: down.length } })}</span>
    <button class="lp-btn sm" onclick={() => (statusFilter = 'offline')}>{$_('fleet.down_show')}</button>
  </div>
{/if}

{#if bufferingCount > 0}
  <!-- Le tampon mérite un bandeau, pas une colonne de plus : il annonce un
       trou dans les données que les courbes ne montreront que plus tard. -->
  <div class="lp-banner" role="status">
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><circle cx="5" cy="5" r="3.7" fill="none" stroke="var(--st-warn)" stroke-width="1.7" /></svg>
    <span class="grow">
      <strong>{$_('buffer.warn_title', { values: { n: bufferingCount } })}</strong>
      · {$_('buffer.warn_body')}
    </span>
    <button class="lp-btn sm" onclick={() => (statusFilter = 'buffering')}>
      {$_('buffer.warn_cta')}
    </button>
  </div>
{/if}

<div class="filters">
  <input
    class="lp-input search"
    type="search"
    bind:value={search}
    placeholder={$_('fleet.search_placeholder')}
    aria-label={$_('common.search')}
  />

  <div class="lp-seg" role="group" aria-label={$_('fleet.col_status')}>
    {#each ['all', 'problems', 'buffering', 'realtime'] as const as f (f)}
      <button aria-pressed={statusFilter === f} onclick={() => (statusFilter = f)}>
        {$_(`fleet.filter_${f}`)}
      </button>
    {/each}
  </div>

  <label>
    <span class="lp-sr">{$_('fleet.site_label')}</span>
    <select
      value={$fleet.siteId ?? ''}
      onchange={(e) => fleet.selectSite(e.currentTarget.value || null, onExpired)}
    >
      <option value="">{$_('fleet.sites_all')}</option>
      {#each $fleet.sites as s (s.site_id)}
        <option value={s.site_id}>{s.name}</option>
      {/each}
    </select>
  </label>

  <span class="grow"></span>

  <label>
    <span class="lp-sr">{$_('fleet.sort')}</span>
    <select bind:value={sort}>
      <option value="status">{$_('fleet.sort_status')}</option>
      <option value="name">{$_('fleet.sort_name')}</option>
      <option value="last_seen">{$_('fleet.sort_last_seen')}</option>
    </select>
  </label>
</div>

{#if $fleet.loading}
  <StateBlock tone="loading" title={$_('fleet.loading')} />
{:else if $fleet.error}
  <StateBlock
    tone="error"
    title={$_('fleet.error_title')}
    body={$fleet.error.isNetwork ? $_('auth.unreachable') : $fleet.error.message}
  >
    <button class="lp-btn primary" onclick={() => void fleet.load(onExpired)}>
      {$_('common.retry')}
    </button>
  </StateBlock>
{:else if $fleet.sites.length === 0 && $fleet.probes.length === 0}
  <!-- Premier vide : il n'y a pas encore de site où rattacher un code, la
       seule action possible est d'en créer un. -->
  <StateBlock title={$_('fleet.empty_sites_title')} body={$_('fleet.empty_sites_body')}>
    {#if $canOperate}
      <button class="lp-btn primary" onclick={() => (createOpen = true)}>
        {$_('fleet.empty_sites_cta')}
      </button>
    {/if}
  </StateBlock>
{:else if filtered.length === 0 && hasFilter}
  <StateBlock
    title={$_('fleet.no_match_title')}
    body={$_('fleet.no_match_body', { values: { n: $fleet.probes.length } })}
  >
    <button
      class="lp-btn"
      onclick={() => {
        search = '';
        statusFilter = 'all';
      }}
    >
      {$_('fleet.no_match_clear')}
    </button>
  </StateBlock>
{:else}
  <div class="lp-tbl-wrap" class:dim={$fleet.refreshing}>
    <table class="lp-t">
      <thead>
        <tr>
          <th>{$_('fleet.col_probe')}</th>
          <th>{$_('fleet.col_status')}</th>
          <th class="opt-s">{$_('fleet.col_strip')}</th>
          <th class="num opt">{$_('fleet.col_latency')}</th>
          <th class="num opt">{$_('fleet.col_loss')}</th>
          <th class="num">{$_('fleet.col_last_seen')}</th>
          <th class="opt-l">{$_('fleet.col_public_ip')}</th>
          <th class="num">{$_('fleet.col_version')}</th>
          <th class="act"><span class="lp-sr">{$_('fleet.col_open')}</span></th>
        </tr>
      </thead>
      {#each groups as g (g.site.site_id)}
        <SiteSection
          siteId={g.site.site_id}
          siteName={g.site.name}
          probes={g.probes}
          newest={newestVersionSeen}
          now={$now}
          beats={$fleetBeats}
          cols={9}
          expanded={isExpanded(g.site.site_id, g.probes, groups.length)}
          ontoggle={() =>
            (manual = {
              ...manual,
              [g.site.site_id]: !isExpanded(g.site.site_id, g.probes, groups.length),
            })}
          onrename={() => openRename(g.site)}
          onalerts={() => openAlerts(g.site.site_id, g.site.name)}
          archived={!!g.site.archived_at}
          onarchive={() =>
            (archiveTarget = {
              id: g.site.site_id,
              name: g.site.name,
              archived: !g.site.archived_at,
            })}
          onexportSla={() => openSlaExport(g.site.site_id, g.site.name)}
          onfocusSite={() => fleet.selectSite(g.site.site_id, onExpired)}
          filtered={hasFilter}
        />
      {/each}
    </table>
  </div>

  <div class="foot">
    <div class="lp-legend">
      <span><i class="lp-sw ok"></i>{$_('fleet.legend_ok')}</span>
      <span><i class="lp-sw w"></i>{$_('fleet.legend_warn')}</span>
      <span><i class="lp-sw b"></i>{$_('fleet.legend_bad')}</span>
      <span><i class="lp-sw n"></i>{$_('fleet.legend_none')}</span>
      <span><i class="lp-sw a"></i>{$_('fleet.legend_realtime')}</span>
    </div>
    {#if $fleet.loadedAt}
      <span class="upd">
        {$_('fleet.updated', {
          values: { when: relativeTime(Math.floor($fleet.loadedAt / 1000), lang, $now) },
        })}
      </span>
    {/if}
    <button
      class="lp-btn ghost sm"
      onclick={() => void fleet.load(onExpired, { quiet: true })}
      disabled={$fleet.refreshing}
    >
      {$fleet.refreshing ? $_('fleet.refreshing') : $_('fleet.refresh')}
    </button>
  </div>
{/if}

<Modal
  open={slaSiteId !== ''}
  wide
  title={$_('sla.export_title', { values: { name: slaSiteName } })}
  onclose={closeSlaExport}
>
  <p class="slalead">{$_('sla.export_lead')}</p>

  <label class="lp-field">
    {$_('probe.range')}
    <select class="lp-input" bind:value={slaRange}>
      {#each RANGES as r (r)}
        <option value={r}>{$_(`probe.range_${r.replace('-', '')}`)}</option>
      {/each}
      <!-- ⚠️ Au-delà de sept jours, et pour tout rapport contractuel, ce sont
           des dates qu'il faut : une fenêtre glissante donne un chiffre
           différent à chaque ouverture du document. -->
      <option value="custom">{$_('sla.custom')}</option>
    </select>
  </label>

  {#if slaRange === 'custom'}
    <div class="sladates">
      <label class="lp-field">
        {$_('sla.from')}
        <input class="lp-input" type="date" bind:value={slaFrom} max={slaTo || undefined} />
      </label>
      <label class="lp-field">
        {$_('sla.to')}
        <input class="lp-input" type="date" bind:value={slaTo} min={slaFrom || undefined} />
      </label>
    </div>
  {/if}

  <!-- ⚠️ Toutes cochées d'office : le cas courant est « tout le site », et
       décocher deux lignes va plus vite que d'en cocher trente. Mais on peut
       retirer une sonde — un rapport qui montre la sonde d'un autre client
       serait pire qu'inutile. -->
  <ul class="slapick">
    {#each slaCandidates as p (p.probe_id)}
      <li>
        <label>
          <input
            type="checkbox"
            checked={slaPicked.has(p.probe_id)}
            onchange={() => togglePicked(p.probe_id)}
          />
          <span>{p.name}</span>
          <!-- Même verdict que la ligne du parc : voir « En ligne » ici sur
               une sonde affichée « Silencieuse » deux clics plus tôt ferait
               douter du rapport avant même de l'ouvrir. -->
          <span class="slastat">{$_(`status.${livenessOf(p, $now)}`)}</span>
        </label>
      </li>
    {/each}
  </ul>

  <!-- ⚠️ L'avancement du hub, en clair. Il ne prépare qu'un rapport à la fois
       pour tout le parc : sur trente sondes l'attente se compte en dizaines de
       secondes, et un bouton qui ne répond pas se lit comme une panne. -->
  {#if slaBusy && slaStep}<p class="slastep" role="status">{slaStep}</p>{/if}
  {#if slaFallback}<p class="slastep">{$_('report.hub_fallback')}</p>{/if}
  {#if slaError}<p class="slaerr" role="alert">{slaError}</p>{/if}

  <!-- 🔴 L'historique : qui a extrait les données de ce client, quand, sur
       quelle fenêtre et quelles sondes. Le fichier part à la purge ; la ligne,
       jamais. Une ligne sans bouton de téléchargement DIT pourquoi. -->
  <section class="slahist">
    <h4>{$_('report.hub_history')}</h4>
    {#if slaHistoryError}
      <p class="slaerr" role="alert">{slaHistoryError}</p>
    {:else if slaHistory.length === 0}
      <p class="slahist-empty">{$_('report.hub_empty')}</p>
    {:else}
      <ul>
        {#each slaHistory as row (row.report.report_id)}
          <li>
            <div class="slahist-main">
              <span class="slahist-when">{absoluteTime(row.report.requested_at, lang)}</span>
              <!-- « depuis le hub web » quand aucun appareil n'est en cause :
                   `device_id` absent ne veut pas dire « inconnu ». -->
              <span class="slahist-who">
                {$_('report.hub_requested_by', { values: { actor: row.report.requested_by } })}
                {#if row.report.device_id == null}· {$_('report.hub_requested_from_web')}{/if}
              </span>
              <span class="slahist-what">
                {$_('report.hub_probes', { values: { n: row.report.probe_ids.length } })} ·
                {absoluteTime(row.report.range_start, lang)} → {absoluteTime(
                  row.report.range_stop,
                  lang,
                )}
              </span>
            </div>
            <div class="slahist-state">
              {#if row.downloadable}
                <span class="slahist-note">
                  {humanBytes(row.report.file_size, lang)}
                  {#if row.report.expires_at != null}
                    · {$_('report.hub_expires_at', {
                      values: { date: absoluteTime(row.report.expires_at, lang) },
                    })}
                  {/if}
                </span>
                <button
                  class="lp-btn ghost sm"
                  disabled={slaRedownloading !== ''}
                  onclick={() =>
                    redownloadReport(row.report.report_id, row.report.file_name ?? '')}
                >
                  {slaRedownloading === row.report.report_id
                    ? $_('sla.exporting')
                    : $_('report.hub_download')}
                </button>
              {:else if row.purged}
                <!-- ⚠️ « Fichier purgé le … », jamais une erreur ni un bouton
                     qui échoue en silence : le rapport a existé, quelqu'un l'a
                     eu, et le bon geste est de le redemander. -->
                <span class="slahist-note">
                  {$_('report.hub_purged_hint', {
                    values: { date: absoluteTime(row.report.purged_at, lang) },
                  })}
                </span>
              {:else if row.failed}
                <span class="slahist-failed">
                  {$_('report.hub_failed', { values: { cause: row.report.error ?? '—' } })}
                </span>
              {:else if row.preparing}
                <span class="slahist-note">
                  {row.report.step ?? $_('report.hub_preparing')}
                </span>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  {#snippet footer()}
    <button class="lp-btn" onclick={closeSlaExport}>{$_('common.cancel')}</button>
    <button
      class="lp-btn primary"
      onclick={runSlaExport}
      disabled={slaBusy || slaPicked.size === 0 || !slaReady}
    >
      {slaBusy ? $_('sla.exporting') : $_('sla.export')}
    </button>
  {/snippet}
</Modal>

<Modal
  open={archiveTarget !== null}
  title={archiveTarget?.archived
    ? $_('site.archive_title', { values: { name: archiveTarget?.name ?? '' } })
    : $_('site.unarchive_title', { values: { name: archiveTarget?.name ?? '' } })}
  onclose={() => (archiveTarget = null)}
>
  <p class="dlg-lead">
    {archiveTarget?.archived ? $_('site.archive_body') : $_('site.unarchive_body')}
  </p>
  {#if archiveTarget?.archived}
    <!-- Ce que l'archivage NE fait pas est aussi important que ce qu'il fait :
         sans cette phrase, on hésite à cliquer de peur de perdre l'historique
         d'un client, et on garde le site pour toujours. -->
    <p class="dlg-note">{$_('site.archive_keeps')}</p>
  {/if}
  {#if archiveError}<p class="dlg-err" role="alert">{archiveError}</p>{/if}
  {#snippet footer()}
    <button class="lp-btn" onclick={() => (archiveTarget = null)}>{$_('common.cancel')}</button>
    <button class="lp-btn primary" onclick={submitArchive} disabled={archiveBusy}>
      {archiveTarget?.archived ? $_('site.archive_confirm') : $_('site.unarchive_confirm')}
    </button>
  {/snippet}
</Modal>

<Modal
  open={alertsSiteId !== ''}
  title={$_('notify.site_alerts_for', { values: { name: alertsSiteName } })}
  onclose={() => (alertsSiteId = '')}
>
  <!-- ⚠️ `{#key}` : sans lui, rouvrir le panneau sur un AUTRE site réutilise
       le composant déjà monté, qui garde les abonnements du premier — on
       réglerait les alertes d'un site en croyant régler celles d'un autre. -->
  {#key alertsSiteId}
    <SiteAlerts siteId={alertsSiteId} {onExpired} />
  {/key}
  {#snippet footer()}
    <button class="lp-btn" onclick={() => (alertsSiteId = '')}>{$_('common.close')}</button>
  {/snippet}
</Modal>

<Modal open={createOpen} title={$_('site.create_title')} onclose={() => (createOpen = false)}>
  <label class="lp-field">
    {$_('site.name_label')}
    <input
      class="lp-input"
      bind:value={createName}
      placeholder={$_('site.name_placeholder')}
      onkeydown={(e) => e.key === 'Enter' && submitCreate()}
    />
  </label>
  {#if createError}<p class="dlg-err" role="alert">{createError}</p>{/if}
  {#snippet footer()}
    <button class="lp-btn" onclick={() => (createOpen = false)}>{$_('common.cancel')}</button>
    <button class="lp-btn primary" onclick={submitCreate} disabled={createBusy}>
      {createBusy ? $_('site.creating') : $_('site.create_submit')}
    </button>
  {/snippet}
</Modal>

<Modal
  open={!!renameTarget}
  title={$_('site.settings_title', { values: { name: renameTarget?.name ?? '' } })}
  onclose={() => (renameTarget = null)}
>
  <label class="lp-field">
    {$_('site.name_label')}
    <input
      class="lp-input"
      bind:value={renameName}
      onkeydown={(e) => e.key === 'Enter' && submitRename()}
    />
  </label>

  <!-- 🔴 La langue du classeur appartient au CLIENT, pas à l'opérateur : c'est
       ce qui garantit qu'un même client reçoive toujours ses rapports dans la
       même langue, quel que soit celui qui appuie sur le bouton. -->
  <label class="lp-field">
    {$_('site.report_locale_label')}
    <select class="lp-input" bind:value={renameLocale}>
      <option value="">{$_('site.report_locale_default')}</option>
      {#each LANGS as l (l)}
        <option value={l}>{$_(`common.lang_${l}`)}</option>
      {/each}
    </select>
  </label>
  <p class="slalead">{$_('site.report_locale_hint')}</p>

  <!-- Le détail qui génère un ticket si on le tait : les points déjà écrits
       gardent l'ancienne étiquette dans Grafana. -->
  <div class="warn">
    <strong>{$_('site.rename_warning_title')}</strong>
    <p>{$_('site.rename_warning_body', { values: { name: renameTarget?.name ?? '' } })}</p>
  </div>

  {#if renameError}<p class="dlg-err" role="alert">{renameError}</p>{/if}
  {#snippet footer()}
    <button class="lp-btn" onclick={() => (renameTarget = null)}>{$_('common.cancel')}</button>
    <button class="lp-btn primary" onclick={submitRename} disabled={renameBusy}>
      {renameBusy ? $_('site.settings_saving') : $_('site.settings_submit')}
    </button>
  {/snippet}
</Modal>

<style>
  .slalead {
    margin: 0 0 12px;
    font-size: 13px;
    color: var(--ep-text-secondary);
  }
  .sladates {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
  }
  .slapick {
    list-style: none;
    margin: 12px 0 0;
    padding: 0;
    max-height: 280px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .slapick label {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border-radius: 5px;
    font-size: 13px;
    cursor: pointer;
  }
  .slapick label:hover {
    background: var(--ep-bg-tertiary);
  }
  .slastat {
    margin-left: auto;
    font-size: 11px;
    color: var(--ep-text-dim);
  }
  .slaerr {
    margin: 10px 0 0;
    font-size: 12px;
    color: var(--ep-danger);
  }

  /* L'avancement du hub : une note, pas une alerte. Rien ne va mal — le
     classeur se prépare, et le dire évite qu'on reclique. */
  .slastep {
    margin: 10px 0 0;
    font-size: 12px;
    color: var(--ep-text-secondary);
  }

  /* L'historique : une trace, pas une liste de fichiers. Il vit sous le
     formulaire parce qu'on le consulte après avoir cherché à produire — et
     qu'il répond souvent à la question « ne l'aurais-je pas déjà sorti ? ». */
  .slahist {
    margin-top: 18px;
    border-top: 1px solid var(--ep-border);
    padding-top: 12px;
  }
  .slahist h4 {
    margin: 0 0 8px;
    font-size: 12px;
    font-weight: 600;
    color: var(--ep-text-secondary);
  }
  .slahist-empty {
    margin: 0;
    font-size: 12px;
    color: var(--ep-text-dim);
  }
  .slahist ul {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 220px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .slahist li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 8px;
    border-radius: 5px;
    background: var(--ep-bg-tertiary);
  }
  .slahist-main {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }
  .slahist-when {
    font-size: 12.5px;
    color: var(--ep-text-primary);
  }
  .slahist-who,
  .slahist-what {
    font-size: 11px;
    color: var(--ep-text-dim);
  }
  .slahist-state {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
    text-align: right;
  }
  .slahist-note {
    font-size: 11px;
    color: var(--ep-text-dim);
    max-width: 260px;
  }
  /* Un échec porte sa cause NOMMÉE : c'est elle qui dit s'il faut réessayer
     ou appeler. */
  .slahist-failed {
    font-size: 11px;
    color: var(--ep-danger);
    max-width: 260px;
  }

  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    margin-bottom: 16px;
  }
  .search {
    flex: 1 1 240px;
    max-width: 360px;
  }
  .grow {
    flex: 1;
  }
  .lp-tbl-wrap {
    transition: opacity 0.15s;
  }
  .lp-tbl-wrap.dim {
    opacity: 0.6;
  }
  .foot {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 24px;
    margin-top: 12px;
  }
  .upd {
    font-size: 0.8125rem;
    color: var(--color-muted);
  }
  @media (max-width: 90rem) {
    .opt-l {
      display: none;
    }
  }
  @media (max-width: 48rem) {
    .opt,
    .opt-s {
      display: none;
    }
  }

  .warn {
    border-left: 2px solid var(--ep-warning);
    padding-left: 10px;
  }
  .warn strong {
    font-size: 12px;
    color: var(--ep-warning);
  }
  .warn p {
    margin: 4px 0 0;
    font-size: 11.5px;
    line-height: 1.55;
  }
  .dlg-err {
    font-size: 12px;
    color: var(--ep-danger);
    margin: 0;
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
  }

</style>
