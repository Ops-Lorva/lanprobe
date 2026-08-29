<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import { RANGES, type Range } from '$lib/metrics';
  import { _, locale } from 'svelte-i18n';
  import { fleet } from '$lib/fleet';
  import { flash } from '$lib/flash';
  import { now, relativeTime } from '$lib/time';
  import { api, ApiError, type Probe, type ProbeStatus } from '$lib/api';
  import { enrollments, pendingRows, pendingKey, type PendingRow } from '$lib/enroll';
  import FleetRail from '$lib/components/FleetRail.svelte';
  import SiteSection from '$lib/components/SiteSection.svelte';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import StatusMark from '$lib/components/StatusMark.svelte';
  import Modal from '$lib/components/Modal.svelte';

  const { onExpired } = $props<{ onExpired: () => void }>();

  const lang = $derived($locale ?? 'en');

  // ── Filtres (une seule barre, au-dessus de tout ce qu'elle cadre) ─────────
  let search = $state('');
  let statusFilter = $state<ProbeStatus | 'all' | 'buffering'>('all');
  let sort = $state<'status' | 'name' | 'last_seen'>('status');

  const STATUS_ORDER: Record<ProbeStatus, number> = { offline: 0, stale: 1, online: 2 };

  const filtered = $derived.by(() => {
    const q = search.trim().toLowerCase();
    return $fleet.probes.filter((p) => {
      if (statusFilter === 'buffering' && p.buffered_points <= 0) return false;
      if (statusFilter !== 'all' && statusFilter !== 'buffering' && p.status !== statusFilter)
        return false;
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
          STATUS_ORDER[a.status] - STATUS_ORDER[b.status] ||
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

    const severity = (probes: Probe[]) =>
      probes.some((p) => p.status === 'offline')
        ? 0
        : probes.some((p) => p.status === 'stale')
          ? 1
          : probes.some((p) => p.buffered_points > 0)
            ? 2
            : 3;

    out.sort((a, b) =>
      sort === 'status'
        ? severity(a.probes) - severity(b.probes) || a.site.name.localeCompare(b.site.name, lang)
        : a.site.name.localeCompare(b.site.name, lang),
    );
    return out;
  });

  const totals = $derived.by(() => {
    const t = { online: 0, stale: 0, offline: 0, buffered: 0 };
    for (const p of $fleet.probes) {
      t[p.status]++;
      t.buffered += p.buffered_points || 0;
    }
    return t;
  });

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
    return probes.some((p) => p.status !== 'online' || p.buffered_points > 0);
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
  let slaSiteId = $state('');
  let slaSiteName = $state('');
  let slaPicked = $state(new SvelteSet<string>());
  let slaRange = $state<Range>('-7d');
  let slaBusy = $state(false);
  let slaError = $state('');

  const slaCandidates = $derived(
    $fleet.probes.filter((p) => p.site_id === slaSiteId),
  );

  function openSlaExport(siteId: string, siteName: string) {
    slaSiteId = siteId;
    slaSiteName = siteName;
    slaError = '';
    // Toutes cochées par défaut : le cas courant est « tout le site », et
    // décocher est plus rapide que cocher trente lignes.
    slaPicked = new SvelteSet(
      $fleet.probes.filter((p) => p.site_id === siteId).map((p) => p.probe_id),
    );
  }

  function togglePicked(probeId: string) {
    if (slaPicked.has(probeId)) slaPicked.delete(probeId);
    else slaPicked.add(probeId);
  }

  async function runSlaExport() {
    slaBusy = true;
    slaError = '';
    try {
      const chosen = slaCandidates.filter((p) => slaPicked.has(p.probe_id));
      // Séquentiel : trente sondes en parallèle, ce sont trente requêtes Flux
      // simultanées sur le même Influx, et un hub qui ne répond plus pendant
      // qu'on produit un rapport.
      const payloads = [];
      for (const p of chosen) {
        payloads.push(await api.sla(p.probe_id, slaRange));
      }
      const { downloadSlaWorkbook } = await import('$lib/sla-report');
      await downloadSlaWorkbook(payloads, slaSiteName, (k, v) => $_(k, { values: v }), lang);
      slaSiteId = '';
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      slaError = e instanceof ApiError ? e.message : String(e);
    } finally {
      slaBusy = false;
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
    <button class="lp-btn sm" onclick={() => (createOpen = true)}>{$_('fleet.new_site')}</button>
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
    stale={totals.stale}
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
    {#each ['offline', 'stale', 'online'] as s (s)}
      <button
        class="chip {s}"
        class:on={statusFilter === s}
        onclick={() => (statusFilter = s as ProbeStatus)}
      >
        <StatusMark status={s as ProbeStatus} size={9} withLabel={false} />
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
    <button class="lp-btn primary" onclick={() => (createOpen = true)}>
      {$_('fleet.empty_sites_cta')}
    </button>
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
  onclose={() => (slaSiteId = '')}
>
  <p class="slalead">{$_('sla.export_lead')}</p>

  <label class="lp-field">
    {$_('probe.range')}
    <select class="lp-input" bind:value={slaRange}>
      {#each RANGES as r (r)}
        <option value={r}>{$_(`probe.range_${r.replace('-', '')}`)}</option>
      {/each}
    </select>
  </label>

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
          <span class="slastat">{$_(`status.${p.status}`)}</span>
        </label>
      </li>
    {/each}
  </ul>

  {#if slaError}<p class="slaerr" role="alert">{slaError}</p>{/if}

  {#snippet footer()}
    <button class="lp-btn" onclick={() => (slaSiteId = '')}>{$_('common.cancel')}</button>
    <button
      class="lp-btn primary"
      onclick={runSlaExport}
      disabled={slaBusy || slaPicked.size === 0}
    >
      {slaBusy ? $_('sla.exporting') : $_('sla.export')}
    </button>
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
