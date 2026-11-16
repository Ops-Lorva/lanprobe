<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { _, locale } from 'svelte-i18n';
  import { api, ApiError } from '$lib/api';
  import { fleet } from '$lib/fleet';
  import { enrollments, pendingRows, pendingKey, type PendingRow } from '$lib/enroll';
  import { pairingAddress } from '$lib/devices';
  import { canOperate, isAdmin } from '$lib/session';
  import { humanBytes } from '$lib/time';
  import {
    detectPlatform,
    packageRows,
    packagesApi,
    parsePackages,
    type PackagePlatform,
    type PackagesPayload,
  } from '$lib/packages';
  import CopyLine from '$lib/components/CopyLine.svelte';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import { go } from '$lib/router';

  /**
   * « Ajouter une sonde » — tout ce qu'il faut pour poser une sonde tient sur
   * un écran : le code, l'adresse du hub, l'installeur.
   *
   * 🔴 L'ancien « Paquets » est retiré, ses fonctions sont ici : la liste des
   * paquets, la récupération signée depuis GitHub, le téléchargement servi par
   * le hub, la page de version. Le nombre de versions gardées reste réglable
   * dans Réglages → Mesures, et cet écran y renvoie.
   *
   * ⚠️ L'enrôlement ne dépend pas de l'installeur : un hub sans Internet
   * affiche son code comme d'habitude, seul le panneau de droite le dit.
   */
  const { site: initialSite, onExpired } = $props<{
    site: string | null;
    onExpired: () => void;
  }>();

  const lang = $derived($locale ?? 'en');

  // ── Site et code ─────────────────────────────────────────────────────────
  let siteId = $state('');
  $effect(() => {
    // Le site vient du « + » de sa ligne ; à défaut, le premier du parc.
    if (siteId || $fleet.sites.length === 0) return;
    const wanted = $fleet.sites.find((s) => s.site_id === initialSite);
    siteId = (wanted ?? $fleet.sites[0]).site_id;
  });
  const siteName = $derived($fleet.sites.find((s) => s.site_id === siteId)?.name ?? '');

  let busy = $state(false);
  let error = $state('');
  /** Le code émis par CET écran, pour le site choisi. */
  let issued = $state<Record<string, string>>({});

  /** Le dernier code encore lisible pour ce site : celui qu'on vient d'émettre. */
  const current = $derived<PendingRow | null>(
    $pendingRows
      .filter((r) => r.site_id === siteId && r.probe_id == null && r.code)
      .find((r) => r.key === issued[siteId]) ?? null,
  );

  async function newCode() {
    if (busy || !siteId) return;
    busy = true;
    error = '';
    try {
      const created = await api.createEnrollCode(siteId);
      const entry = {
        site_id: created.site_id ?? siteId,
        site: created.site ?? siteName,
        probe_id: null,
        probe: null,
        created_at: Math.floor(Date.now() / 1000),
        expires_at: created.expires_at,
      };
      enrollments.remember(entry, created.code);
      issued = { ...issued, [siteId]: pendingKey(entry) };
      void enrollments.refresh(onExpired);
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      error = e instanceof ApiError ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  /**
   * Remplacer un code en attente : nouvelle sonde → nouveau code de site ;
   * réparation → code rattaché à la sonde, pour qu'elle revienne avec son
   * historique.
   */
  let rowBusy = $state('');
  async function regenerate(row: PendingRow) {
    if (rowBusy) return;
    rowBusy = row.key;
    error = '';
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
      // L'ancienne ligne sort de l'écran : deux emplacements pour une seule
      // machine se liraient comme deux sondes à installer. Son code n'est pas
      // révoqué pour autant — il expire tout seul.
      if (pendingKey(next) !== row.key) enrollments.hide(row.key);
      void enrollments.refresh(onExpired);
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      error = e instanceof ApiError ? e.message : String(e);
    } finally {
      rowBusy = '';
    }
  }

  // Horloge à la seconde : le compte à rebours se lit pendant qu'on dicte.
  let tick = $state(Date.now());
  function left(expiresAt: number): string {
    const s = Math.max(0, Math.floor(expiresAt - tick / 1000));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  }
  /** Le code se lit à voix haute : deux paquets de quatre. */
  function grouped(code: string): string {
    const raw = code.replace(/[^A-Za-z0-9]/g, '').toUpperCase();
    return (raw.match(/.{1,4}/g) ?? [raw]).join('-');
  }

  // ── Adresse du hub ───────────────────────────────────────────────────────
  // ⚠️ L'adresse publique réglée prime sur celle de la page : `location.origin`
  // peut être une IP interne que la sonde ne joindra pas.
  let hubUrl = $state('');

  // ── Installeur ───────────────────────────────────────────────────────────
  let payload = $state<PackagesPayload | null>(null);
  let pkgLoading = $state(true);
  let pkgError = $state('');
  let fetching = $state<PackagePlatform | null>(null);
  let fetchError = $state('');
  const detected = $derived(
    detectPlatform(typeof navigator === 'undefined' ? '' : navigator.userAgent),
  );
  const rows = $derived(payload ? packageRows(payload, detected) : []);
  const FORMAT: Record<PackagePlatform, string> = {
    windows: '.exe',
    macos: '.pkg',
    debian: '.deb',
    'debian-headless': '.deb',
  };

  async function loadPackages() {
    pkgLoading = true;
    pkgError = '';
    try {
      payload = parsePackages(await packagesApi.list());
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      payload = null;
      pkgError = e instanceof ApiError ? e.message : String(e);
    } finally {
      pkgLoading = false;
    }
  }

  /**
   * ⚠️ Deux temps, et c'est voulu : le hub sort, tire le fichier et vérifie sa
   * signature — puis le navigateur le télécharge depuis le hub. Un seul clic
   * qui ferait les deux resterait muet plusieurs minutes.
   */
  async function fetchPackage(platform: PackagePlatform) {
    fetching = platform;
    fetchError = '';
    try {
      await packagesApi.fetch(platform);
      await loadPackages();
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      fetchError = e instanceof ApiError ? e.message : String(e);
    } finally {
      fetching = null;
    }
  }

  let timer: ReturnType<typeof setInterval> | null = null;
  let refreshTimer: ReturnType<typeof setInterval> | null = null;
  onMount(() => {
    if ($fleet.sites.length === 0) void fleet.load(onExpired);
    void enrollments.refresh(onExpired);
    void loadPackages();
    api
      .settings()
      .then((s) => (hubUrl = pairingAddress(s.hub_public_url, location.origin)))
      .catch(() => (hubUrl = pairingAddress(null, location.origin)));
    timer = setInterval(() => (tick = Date.now()), 1000);
    // Une place réservée disparaît d'elle-même quand la machine a consommé
    // son code : on relit les attentes comme le faisait le parc.
    refreshTimer = setInterval(() => void enrollments.refresh(onExpired), 30_000);
  });
  onDestroy(() => {
    if (timer) clearInterval(timer);
    if (refreshTimer) clearInterval(refreshTimer);
  });
</script>

<nav class="lp-crumbs" aria-label={$_('nav.fleet')}><a href="#/">{$_('nav.fleet')}</a></nav>
<header class="lp-head"><h1>{$_('add.title')}</h1></header>

{#if !$canOperate}
  <StateBlock title={$_('add.forbidden')} />
{:else if $fleet.loading && $fleet.sites.length === 0}
  <StateBlock tone="loading" title={$_('common.loading')} />
{:else if $fleet.sites.length === 0}
  <StateBlock title={$_('fleet.empty_sites_title')} body={$_('fleet.empty_sites_body')}>
    <button class="lp-btn primary" onclick={() => go('/')}>{$_('add.go_fleet')}</button>
  </StateBlock>
{:else}
  <div class="grid2">
    <section class="lp-card panel lp-ticks">
      <h2>{$_('add.code_title')}</h2>
      <label class="lp-field site">
        {$_('fleet.site_label')}
        <select bind:value={siteId}>
          {#each $fleet.sites.filter((s) => !s.archived_at) as s (s.site_id)}
            <option value={s.site_id}>{s.name}</option>
          {/each}
        </select>
      </label>

      {#if current?.code}
        <div class="copyline">
          <span class="bigcode" aria-label={$_('pending.aria_new', { values: { code: current.code } })}>
            {grouped(current.code)}
          </span>
          <CopyLine value={current.code} shell={false} hideValue compact />
        </div>
        <p class="muted">{$_('add.expires', { values: { left: left(current.expires_at) } })}</p>
      {/if}

      <div class="lp-field hub">
        <span>{$_('add.hub_address')}</span>
        <div class="copyline">
          <code>{hubUrl}</code>
          {#if hubUrl}<CopyLine value={hubUrl} shell={false} hideValue compact />{/if}
        </div>
      </div>

      {#if error}<p class="err" role="alert">{error}</p>{/if}
      <div>
        <button
          class="lp-btn"
          class:primary={!current}
          onclick={newCode}
          disabled={busy || !siteId}
          aria-busy={busy}
        >
          {busy ? $_('add.generating') : current ? $_('add.new_code') : $_('add.generate')}
        </button>
      </div>
    </section>

    <section class="lp-card panel">
      <div class="ph">
        <h2>{$_('add.installer')}</h2>
        {#if payload?.version}<span class="muted">{$_('add.version', { values: { version: payload.version } })}</span>{/if}
        {#if payload?.notes_url}
          <a class="lp-link notes" href={payload.notes_url} target="_blank" rel="noopener noreferrer">
            {$_('add.release_notes')}
          </a>
        {/if}
      </div>

      {#if pkgLoading}
        <p class="muted" role="status">{$_('packages.loading')}</p>
      {:else if pkgError}
        <p class="err" role="alert">{pkgError}</p>
        <button class="lp-btn sm" onclick={loadPackages}>{$_('common.retry')}</button>
      {:else if payload}
        {#if !payload.reachable}
          <!-- ⚠️ Un hub auto-hébergé peut ne pas avoir Internet. On le DIT :
               sans cette phrase, les lignes sans bouton se lisent comme une
               panne. Ce qui est en cache reste servi. -->
          <p class="lp-banner" role="status">
            {$_('packages.offline')}
            {#if payload.error}<span class="lp-mono why">{payload.error}</span>{/if}
          </p>
        {/if}
        <table class="lp-t">
          <thead>
            <tr>
              <th>{$_('add.col_platform')}</th>
              <th>{$_('add.col_format')}</th>
              <th class="num">{$_('add.col_size')}</th>
              <th class="act"><span class="lp-sr">{$_('packages.download')}</span></th>
            </tr>
          </thead>
          <tbody>
            {#each rows as row (row.pkg.platform)}
              <tr>
                <td>
                  {$_(`add.os_${row.pkg.platform}`)}
                  {#if row.suggested}<span class="lp-tag">{$_('add.this_device')}</span>{/if}
                </td>
                <td><code>{FORMAT[row.pkg.platform]}</code></td>
                <td class="num">{row.pkg.size == null ? '' : humanBytes(row.pkg.size, lang)}</td>
                <td class="act">
                  {#if row.state === 'cached'}
                    <!-- Un lien et non un `fetch` : le navigateur enregistre le
                         fichier, et le cookie de session part avec. Rien n'entre
                         dans ce cache sans signature vérifiée. -->
                    <a
                      class="lp-btn sm"
                      class:primary={row.suggested && !!current}
                      href={packagesApi.fileUrl(row.pkg.platform)}
                      download
                      title={$_('packages.cached', {
                        values: {
                          version: row.pkg.version ?? '—',
                          size: row.pkg.size == null ? '—' : humanBytes(row.pkg.size, lang),
                        },
                      })}
                    >
                      <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M8 2v8" /><polyline points="4.5,7 8,10.5 11.5,7" /><path d="M3 13h10" /></svg>
                      {$_('packages.download')}
                    </a>
                  {:else if row.state === 'fetchable'}
                    <button
                      class="lp-btn sm"
                      onclick={() => fetchPackage(row.pkg.platform)}
                      disabled={fetching !== null}
                      aria-busy={fetching === row.pkg.platform}
                    >
                      {fetching === row.pkg.platform ? $_('packages.fetching') : $_('packages.fetch')}
                    </button>
                  {:else if row.state === 'unsigned'}
                    <!-- 🔴 Dit AVANT le clic : un bouton qui échoue à tous les
                         coups est un bouton mort. -->
                    <span class="note warn">{$_('packages.unsigned_short')}</span>
                  {:else if row.state === 'absent'}
                    <span class="note">{$_('packages.absent_short')}</span>
                  {:else}
                    <span class="note">{$_('packages.offline_row')}</span>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
        {#if fetching}
          <p class="muted" role="status">{$_('packages.fetching_note')}</p>
        {/if}
        {#if fetchError}<p class="err" role="alert">{fetchError}</p>{/if}
        {#if payload.keep_versions > 0}
          <p class="muted keep">
            {$_('packages.keep', { values: { n: payload.keep_versions } })}
            {#if $isAdmin}<a class="lp-link" href="#/settings/storage">{$_('add.keep_setting')}</a>{/if}
          </p>
        {/if}
      {/if}
    </section>
  </div>

  {#if $pendingRows.length > 0}
    <div class="lp-sec-head"><h2>{$_('add.pending_title')}</h2></div>
    <div class="lp-tbl-wrap">
      <table class="lp-t">
        <thead>
          <tr>
            <th>{$_('fleet.site_label')}</th>
            <th>{$_('add.col_code')}</th>
            <th>{$_('add.col_for')}</th>
            <th class="num">{$_('add.col_expires')}</th>
            <th class="act"><span class="lp-sr">{$_('add.col_actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          {#each $pendingRows as row (row.key)}
            {@const expired = row.expires_at * 1000 <= tick}
            <tr>
              <td>{row.site}</td>
              <td>
                {#if row.code}<code>{grouped(row.code)}</code>{:else}<span class="muted">{$_('pending.lost_title')}</span>{/if}
              </td>
              <td>
                {row.probe_id
                  ? $_('add.for_reenroll', { values: { name: row.probe ?? '' } })
                  : $_('add.for_new')}
              </td>
              <td class="num">{expired ? $_('pending.expired') : left(row.expires_at)}</td>
              <td class="act">
                <button class="lp-btn ghost sm" onclick={() => regenerate(row)} disabled={rowBusy !== ''}>
                  {$_('add.new_code')}
                </button>
                <!-- « Masquer » et non « Annuler » : le code reste valable,
                     la ligne quitte seulement l'écran. -->
                <button class="lp-btn ghost sm" onclick={() => enrollments.hide(row.key)} title={$_('pending.hide_hint')}>
                  {$_('pending.hide')}
                </button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
{/if}

<style>
  .grid2 {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 16px;
    align-items: start;
  }
  @media (max-width: 64rem) {
    .grid2 {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .panel {
    padding: 16px;
    display: grid;
    gap: 16px;
    min-width: 0;
  }
  .panel h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .site select {
    max-width: 320px;
  }
  .ph {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 16px;
  }
  .notes {
    margin-left: auto;
    font-size: 0.8125rem;
  }
  .copyline {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .bigcode {
    font-family: var(--ep-font-mono);
    font-size: 2rem;
    letter-spacing: 0.08em;
    font-weight: 500;
    line-height: 1.1;
  }
  .muted {
    color: var(--color-muted);
    font-size: 0.875rem;
    margin: -8px 0 0;
  }
  .ph .muted,
  .keep {
    margin: 0;
  }
  code {
    font-family: var(--ep-font-mono);
    font-size: 0.875rem;
  }
  .hub {
    color: var(--color-ink-2);
  }
  .err {
    color: var(--st-bad);
    font-size: 0.8125rem;
    margin: 0;
  }
  .note {
    color: var(--color-muted);
    font-size: 0.8125rem;
    white-space: normal;
  }
  .note.warn {
    color: var(--st-warn);
  }
  .why {
    display: block;
    font-size: 0.75rem;
    color: var(--color-muted);
  }
  .lp-tag {
    margin-left: 8px;
  }
  .panel .lp-t th:first-child,
  .panel .lp-t td:first-child {
    padding-left: 0;
  }
  .keep a {
    margin-left: 8px;
  }
</style>
