<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { canOperate } from '$lib/session';
  import { SvelteSet } from 'svelte/reactivity';
  import { RANGES, type Range } from '$lib/metrics';
  import { _, locale } from 'svelte-i18n';
  import { fleet } from '$lib/fleet';
  import { flash } from '$lib/flash';
  import { absoluteTime, humanBytes, now, relativeTime } from '$lib/time';
  import { api, ApiError, type MetricsWindow, type Probe } from '$lib/api';
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
    LIVENESS_ORDER,
    fleetTotals,
    livenessOf,
    matchesFleetFilter,
    needsAttention,
    siteSeverity,
    sortKey,
    type FleetFilter,
  } from '$lib/fleet-filter';
  import { enrollments, pendingRows, pendingKey, type PendingRow } from '$lib/enroll';
  import FleetRail from '$lib/components/FleetRail.svelte';
  import SiteSection from '$lib/components/SiteSection.svelte';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import StatusMark from '$lib/components/StatusMark.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import SiteAlerts from '$lib/components/SiteAlerts.svelte';

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
  const totals = $derived(fleetTotals($fleet.probes, $now));

  const bufferingCount = $derived($fleet.probes.filter((p) => p.buffered_points > 0).length);
  const hasFilter = $derived(search.trim() !== '' || statusFilter !== 'all');

  /** Attentes rangées par site : elles s'affichent dans la liste de leur site. */
  const pendingBySite = $derived.by(() => {
    const m = new Map<string, PendingRow[]>();
    for (const r of $pendingRows) {
      const list = m.get(r.site_id) ?? [];
      list.push(r);
      m.set(r.site_id, list);
    }
    return m;
  });

  // ── Dépliage ─────────────────────────────────────────────────────────────
  let manual = $state<Record<string, boolean>>({});

  function isExpanded(siteId: string, probes: Probe[], siteCount: number) {
    if (siteId in manual) return manual[siteId];
    // Une attente en cours ouvre toujours son site : un code qu'il faut
    // déplier pour lire est un code qu'on croit perdu.
    if ((pendingBySite.get(siteId)?.length ?? 0) > 0) return true;
    // Peu de sites : tout ouvert, on voit le parc d'un bloc.
    // Beaucoup de sites : seuls ceux qui demandent une action s'ouvrent, les
    // autres restent pliés — mais leur en-tête dit déjà comment ils vont.
    if (siteCount <= 3) return true;
    return needsAttention(probes, $now);
  }

  // ── Enrôlement : le « + » du site crée le code, sur place ────────────────
  // Aucune fenêtre, aucun formulaire : le site est déterminé par l'endroit du
  // clic, et le code n'a rien d'autre à recevoir. Ce qui apparaît est une ligne
  // dans la liste de ce site — un emplacement réservé, lisible au téléphone.
  let addBusySite = $state<string | null>(null);
  let addErrors = $state<Record<string, string>>({});
  let pendingBusyKey = $state<string | null>(null);

  async function addProbe(siteId: string, siteName: string) {
    if (addBusySite) return;
    addBusySite = siteId;
    addErrors = { ...addErrors, [siteId]: '' };
    try {
      const created = await api.createEnrollCode(siteId);
      enrollments.remember(
        {
          site_id: created.site_id ?? siteId,
          site: created.site ?? siteName,
          probe_id: null,
          probe: null,
          created_at: Math.floor(Date.now() / 1000),
          expires_at: created.expires_at,
        },
        created.code,
      );
      // Le site s'ouvre, sinon le code atterrit dans une section repliée.
      manual = { ...manual, [siteId]: true };
      void enrollments.refresh(onExpired);
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      addErrors = {
        ...addErrors,
        [siteId]: e instanceof ApiError ? e.message : String(e),
      };
    } finally {
      addBusySite = null;
    }
  }

  /**
   * Remplacer un code : nouvelle sonde → nouveau code de site ; réparation →
   * code rattaché à la sonde, pour qu'elle revienne avec son historique.
   */
  async function regenerate(row: PendingRow) {
    if (pendingBusyKey) return;
    pendingBusyKey = row.key;
    addErrors = { ...addErrors, [row.site_id]: '' };
    try {
      const created = row.probe_id
        ? await api.reenrollCode(row.probe_id)
        : await api.createEnrollCode(row.site_id);
      const next = {
        site_id: created.site_id ?? row.site_id,
        site: created.site ?? row.site,
        probe_id: row.probe_id,
        probe: row.probe,
        created_at: Math.floor(Date.now() / 1000),
        expires_at: created.expires_at,
      };
      enrollments.remember(next, created.code);
      // L'ancienne ligne sort de l'écran : on vient de demander à la remplacer,
      // et deux emplacements réservés pour une seule machine se lisent comme
      // deux sondes à installer. Son code n'est pas révoqué pour autant — il
      // expire tout seul, personne ne l'a plus sous les yeux.
      if (pendingKey(next) !== row.key) enrollments.hide(row.key);
      void enrollments.refresh(onExpired);
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      addErrors = {
        ...addErrors,
        [row.site_id]: e instanceof ApiError ? e.message : String(e),
      };
    } finally {
      pendingBusyKey = null;
    }
  }

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
  let renameBusy = $state(false);
  let renameError = $state('');

  function openRename(id: string, name: string) {
    renameTarget = { id, name };
    renameName = name;
    renameError = '';
  }

  async function submitRename() {
    if (!renameTarget) return;
    const name = renameName.trim();
    if (!name) return void (renameError = $_('site.empty_name'));
    renameBusy = true;
    renameError = '';
    try {
      await api.renameSite(renameTarget.id, name);
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
        locale: lang,
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

  // ── Cycle de vie ─────────────────────────────────────────────────────────
  let timer: ReturnType<typeof setInterval> | null = null;

  onMount(() => {
    void fleet.load(onExpired);
    void enrollments.refresh(onExpired);
    // Le statut est dérivé du dernier battement : sans rafraîchissement, un
    // parc laissé ouvert affiche un « en ligne » qui a vieilli d'une heure.
    // Le même tour recharge les attentes : c'est ainsi qu'une place réservée
    // disparaît d'elle-même quand la machine a consommé son code.
    timer = setInterval(() => {
      void fleet.load(onExpired, { quiet: true });
      void enrollments.refresh(onExpired);
    }, 30_000);
  });
  onDestroy(() => {
    if (timer) clearInterval(timer);
  });
</script>

{#if $flash}
  <!-- Confirmation rapportée depuis le détail : elle redit que les mesures
       sont conservées, au moment où l'utilisateur constate la ligne en moins. -->
  <div class="flash" role="status">
    <span>{$flash}</span>
    <button class="lp-btn ghost sm" onclick={() => flash.set('')}>{$_('common.close')}</button>
  </div>
{/if}

<header class="page-head">
  <div>
    <h1>{$_('fleet.title')}</h1>
    <p class="sub">
      {$_('fleet.site_count', { values: { n: $fleet.sites.length } })}
      · {$_('fleet.count', { values: { n: $fleet.probes.length } })}
      {#if $fleet.loadedAt}
        <span class="upd">
          · {$_('fleet.updated', {
            values: { when: relativeTime(Math.floor($fleet.loadedAt / 1000), lang, $now) },
          })}
        </span>
      {/if}
    </p>
  </div>
  <div class="head-acts">
    {#if $canOperate}
      <button class="lp-btn sm" onclick={() => (createOpen = true)}>{$_('fleet.new_site')}</button>
    {/if}
    <!-- ⚠️ Le chemin vers les archives ne disparaît jamais, même à zéro
         archive : sans lui, on archive un site puis on cherche comment le
         faire revenir. Le compte s'affiche quand il y en a. -->
    <button
      class="lp-btn sm"
      class:on={$fleet.showArchived}
      onclick={() => fleet.toggleArchived(onExpired)}
      aria-pressed={$fleet.showArchived}
    >
      {$fleet.showArchived
        ? $_('fleet.hide_archived')
        : archivedCount > 0
          ? $_('fleet.show_archived_n', { values: { n: archivedCount } })
          : $_('fleet.show_archived')}
    </button>
    <button
      class="lp-btn sm"
      onclick={() => void fleet.load(onExpired, { quiet: true })}
      disabled={$fleet.refreshing}
    >
      {$fleet.refreshing ? $_('fleet.refreshing') : $_('fleet.refresh')}
    </button>
  </div>
</header>

<div class="rail-wrap">
  <FleetRail
    sites={$fleet.sites.length}
    total={$fleet.probes.length}
    online={totals.online}
    silent={totals.silent}
    offline={totals.offline}
    buffered={totals.buffered}
  />
</div>

{#if bufferingCount > 0}
  <!-- Le tampon mérite un bandeau, pas une colonne de plus : il annonce un
       trou dans les données que les courbes ne montreront que plus tard. -->
  <div class="buffer-alert" role="status">
    <div>
      <strong>{$_('buffer.warn_title', { values: { n: bufferingCount } })}</strong>
      <p>{$_('buffer.warn_body')}</p>
    </div>
    <button class="lp-btn sm" onclick={() => (statusFilter = 'buffering')}>
      {$_('buffer.warn_cta')}
    </button>
  </div>
{/if}

<div class="toolbar">
  <label class="ctl">
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

  <div class="chips" role="group" aria-label={$_('fleet.col_status')}>
    <button class="chip" class:on={statusFilter === 'all'} onclick={() => (statusFilter = 'all')}>
      {$_('fleet.filter_all')}
    </button>
    {#each LIVENESS_ORDER as s (s)}
      <button
        class="chip {s}"
        class:on={statusFilter === s}
        onclick={() => (statusFilter = s as LivenessState)}
      >
        <StatusMark status={s} size={9} withLabel={false} />
        {$_(`status.${s}`)}
      </button>
    {/each}
    <button
      class="chip buffering"
      class:on={statusFilter === 'buffering'}
      onclick={() => (statusFilter = 'buffering')}
    >
      {$_('fleet.filter_buffering')}
    </button>
  </div>

  <input
    class="lp-input search"
    type="search"
    bind:value={search}
    placeholder={$_('fleet.search_placeholder')}
    aria-label={$_('common.search')}
  />

  <label class="ctl">
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
  <!-- Premier vide : rien du tout. Il n'y a pas encore de site où réserver une
       place, donc la seule action possible est d'en créer un. -->
  <StateBlock title={$_('fleet.empty_sites_title')} body={$_('fleet.empty_sites_body')}>
    {#if $canOperate}
      <button class="lp-btn primary" onclick={() => (createOpen = true)}>
        {$_('fleet.empty_sites_cta')}
      </button>
    {/if}
  </StateBlock>
{:else if filtered.length === 0 && hasFilter && $pendingRows.length === 0}
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
  <div class="sections" class:dim={$fleet.refreshing}>
    {#each groups as g (g.site.site_id)}
      <SiteSection
        siteId={g.site.site_id}
        siteName={g.site.name}
        probes={g.probes}
        pending={pendingBySite.get(g.site.site_id) ?? []}
        now={$now}
        expanded={isExpanded(g.site.site_id, g.probes, groups.length)}
        ontoggle={() =>
          (manual = {
            ...manual,
            [g.site.site_id]: !isExpanded(g.site.site_id, g.probes, groups.length),
          })}
        onrename={() => openRename(g.site.site_id, g.site.name)}
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
        onadd={() => void addProbe(g.site.site_id, g.site.name)}
        addBusy={addBusySite === g.site.site_id}
        addError={addErrors[g.site.site_id] ?? ''}
        onregenerate={(row) => void regenerate(row)}
        onhidePending={(row) => enrollments.hide(row.key)}
        {pendingBusyKey}
        filtered={hasFilter}
      />
    {/each}
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
  title={$_('site.rename_title', { values: { name: renameTarget?.name ?? '' } })}
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
      {renameBusy ? $_('site.renaming') : $_('site.rename_submit')}
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

  .flash {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
    margin-bottom: 12px;
    border: 1px solid color-mix(in srgb, var(--ep-success) 45%, var(--ep-border));
    border-left-width: 3px;
    border-radius: var(--ep-radius-md);
    background: color-mix(in srgb, var(--ep-success) 8%, transparent);
    font-size: 12.5px;
    color: var(--ep-text-primary);
  }

  .page-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
    margin-bottom: 14px;
  }
  /*
    Le bloc de titre doit pouvoir rétrécir. Sans ça il réclame sa largeur de
    contenu, et « Actualisé il y a 1 min. » — qui s'allonge tout seul avec le
    temps — pousse les deux boutons sur une deuxième ligne : 40 px de haut de
    page qui apparaissent et disparaissent selon l'âge du rafraîchissement.
    On préfère que la ligne de comptage passe à la ligne, elle, c'est stable.

    Base explicite et non `auto` : avec `flex-wrap`, le navigateur passe à la
    ligne AVANT de rétrécir un élément dont la base vaut son contenu. Une base
    de 180 px laisse les deux boutons tenir sur la même ligne, et c'est la
    ligne de comptage qui se replie.
  */
  .page-head > div:first-child {
    flex: 1 1 180px;
    min-width: 0;
  }
  h1 {
    font-size: 20px;
    font-weight: 800;
    letter-spacing: -0.02em;
    margin: 0;
  }
  .sub {
    font-size: 11.5px;
    color: var(--ep-text-muted);
    margin: 3px 0 0;
  }
  .upd {
    color: var(--ep-text-dim);
  }
  .head-acts {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }

  .rail-wrap {
    margin-bottom: 12px;
  }

  .buffer-alert {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 14px;
    flex-wrap: wrap;
    padding: 12px 14px;
    margin-bottom: 12px;
    border: 1px solid color-mix(in srgb, var(--ep-warning) 50%, var(--ep-border));
    border-left-width: 3px;
    border-radius: var(--ep-radius-md);
    background: color-mix(in srgb, var(--ep-warning) 8%, transparent);
  }
  .buffer-alert strong {
    font-size: 12.5px;
    color: var(--ep-text-primary);
  }
  .buffer-alert p {
    font-size: 11.5px;
    color: var(--ep-text-secondary);
    margin: 4px 0 0;
    max-width: 78ch;
    line-height: 1.55;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-bottom: 12px;
  }
  .search {
    flex: 1 1 190px;
    min-width: 0;
    max-width: 280px;
    font-family: var(--ep-font-sans);
    font-size: 12px;
    min-height: 30px;
    padding: 5px 9px;
  }
  .ctl select {
    font-size: 12px;
  }

  .chips {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 5px 9px;
    border-radius: 999px;
    border: 1px solid var(--ep-border);
    background: var(--ep-bg-secondary);
    color: var(--ep-text-secondary);
    font-family: var(--ep-font-sans);
    font-size: 11.5px;
    cursor: pointer;
  }
  .chip:hover {
    border-color: var(--ep-border-strong);
    color: var(--ep-text-primary);
  }
  .chip.on {
    border-color: var(--ep-accent);
    background: var(--ep-accent-dim);
    color: var(--ep-accent-bright);
    font-weight: 600;
  }
  .chip.buffering.on {
    border-color: var(--ep-warning);
    background: color-mix(in srgb, var(--ep-warning) 14%, transparent);
    color: var(--ep-warning);
  }

  /* Doit rester aligné sur la grille de ProbeRow.svelte — les styles Svelte
     sont scopés, la définition ne peut pas être partagée. */

  .sections {
    display: flex;
    flex-direction: column;
    gap: 10px;
    transition: opacity 0.15s;
  }
  .sections.dim {
    opacity: 0.6;
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

  @media (max-width: 1080px) {
    .c-platform {
      display: none;
    }
  }
  @media (max-width: 900px) {
    .c-version {
      display: none;
    }
  }
  @media (max-width: 640px) {
  }
</style>
