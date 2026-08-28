<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { _, locale } from 'svelte-i18n';
  import { fleet } from '$lib/fleet';
  import { flash } from '$lib/flash';
  import { now, relativeTime } from '$lib/time';
  import { go } from '$lib/router';
  import { api, ApiError, type Probe, type ProbeStatus } from '$lib/api';
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

  // ── Dépliage ─────────────────────────────────────────────────────────────
  let manual = $state<Record<string, boolean>>({});

  function isExpanded(siteId: string, probes: Probe[], siteCount: number) {
    if (siteId in manual) return manual[siteId];
    // Peu de sites : tout ouvert, on voit le parc d'un bloc.
    // Beaucoup de sites : seuls ceux qui demandent une action s'ouvrent, les
    // autres restent pliés — mais leur en-tête dit déjà comment ils vont.
    if (siteCount <= 3) return true;
    return probes.some((p) => p.status !== 'online' || p.buffered_points > 0);
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

  // ── Cycle de vie ─────────────────────────────────────────────────────────
  let timer: ReturnType<typeof setInterval> | null = null;

  onMount(() => {
    void fleet.load(onExpired);
    // Le statut est dérivé du dernier battement : sans rafraîchissement, un
    // parc laissé ouvert affiche un « en ligne » qui a vieilli d'une heure.
    timer = setInterval(() => void fleet.load(onExpired, { quiet: true }), 30_000);
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
  <!-- Premier vide : rien du tout. On raconte d'où viennent les sondes. -->
  <StateBlock title={$_('fleet.empty_sites_title')} body={$_('fleet.empty_sites_body')}>
    <button class="lp-btn primary" onclick={() => go('/enroll')}>
      {$_('fleet.empty_sites_cta')}
    </button>
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
  <!-- En-tête de colonnes : une seule fois pour tout le parc, plutôt que
       répété dans chacun des dix sites. Décoratif pour les lecteurs d'écran,
       chaque ligne portant déjà ses propres libellés. -->
  <div class="cols" aria-hidden="true">
    <span>{$_('fleet.col_probe')}</span>
    <span>{$_('fleet.col_status')}</span>
    <span>{$_('fleet.col_last_seen')}</span>
    <span class="c-platform">{$_('fleet.col_platform')}</span>
    <span class="c-version">{$_('fleet.col_version')}</span>
    <span>{$_('fleet.col_buffer')}</span>
    <span></span>
  </div>

  <div class="sections" class:dim={$fleet.refreshing}>
    {#each groups as g (g.site.site_id)}
      <SiteSection
        siteId={g.site.site_id}
        siteName={g.site.name}
        probes={g.probes}
        now={$now}
        expanded={isExpanded(g.site.site_id, g.probes, groups.length)}
        ontoggle={() =>
          (manual = {
            ...manual,
            [g.site.site_id]: !isExpanded(g.site.site_id, g.probes, groups.length),
          })}
        onrename={() => openRename(g.site.site_id, g.site.name)}
        onfocusSite={() => fleet.selectSite(g.site.site_id, onExpired)}
        filtered={hasFilter}
      />
    {/each}
  </div>
{/if}

<Modal open={createOpen} title={$_('site.create_title')} onclose={() => (createOpen = false)}>
  <p>{$_('site.create_hint')}</p>
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
  .cols {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 118px 120px 92px 84px 78px 18px;
    gap: 10px;
    padding: 0 12px 6px 12px;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--ep-text-dim);
  }

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
    .cols {
      grid-template-columns: minmax(0, 1fr) 118px 120px 84px 78px 18px;
    }
    .c-platform {
      display: none;
    }
  }
  @media (max-width: 900px) {
    .cols {
      grid-template-columns: minmax(0, 1fr) 118px 110px 78px 18px;
    }
    .c-version {
      display: none;
    }
  }
  @media (max-width: 640px) {
    .cols {
      display: none;
    }
  }
</style>
