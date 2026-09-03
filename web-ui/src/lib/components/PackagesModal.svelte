<script lang="ts">
  import { onMount } from 'svelte';
  import { _, locale } from 'svelte-i18n';
  import Modal from './Modal.svelte';
  import { ApiError } from '$lib/api';
  import { humanBytes } from '$lib/time';
  import {
    detectPlatform,
    packageRows,
    packagesApi,
    parsePackages,
    type PackagePlatform,
    type PackagesPayload,
  } from '$lib/packages';

  interface Props {
    open: boolean;
    onclose: () => void;
    onexpired: () => void;
  }
  let { open, onclose, onexpired }: Props = $props();

  const lang = $derived($locale ?? 'en');

  let payload = $state<PackagesPayload | null>(null);
  let loading = $state(true);
  let error = $state('');
  /** La plateforme en cours de récupération — l'écran le DIT pendant ce temps. */
  let fetching = $state<PackagePlatform | null>(null);
  let fetchError = $state('');

  const detected = $derived(
    detectPlatform(typeof navigator === 'undefined' ? '' : navigator.userAgent),
  );
  const rows = $derived(payload ? packageRows(payload, detected) : []);

  async function load() {
    loading = true;
    error = '';
    try {
      payload = parsePackages(await packagesApi.list());
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onexpired();
      payload = null;
      error = e instanceof ApiError ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  onMount(load);

  /**
   * ⚠️ Deux temps, et c'est voulu : le hub sort, tire le fichier et vérifie sa
   * signature — puis le navigateur le télécharge depuis le hub. Un seul clic
   * qui ferait les deux resterait muet plusieurs minutes.
   */
  async function recuperer(platform: PackagePlatform) {
    fetching = platform;
    fetchError = '';
    try {
      await packagesApi.fetch(platform);
      await load();
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onexpired();
      fetchError = e instanceof ApiError ? e.message : String(e);
    } finally {
      fetching = null;
    }
  }
</script>

<!--
  🔴 Une seule fenêtre, appelée par un seul bouton. Quatre boutons sur l'écran
  d'enrôlement noieraient le geste principal, qui reste l'enrôlement.

  ⚠️ Aucune plateforme n'est cachée : on installe souvent la sonde depuis la
  machine où l'on est, mais pas toujours. Le système détecté est marqué, pas
  privilégié — détecter aide, décider à sa place gêne.
-->
<Modal {open} title={$_('packages.title')} {onclose} wide>
  <p class="sub">{$_('packages.lead')}</p>

  {#if loading}
    <p class="muted" role="status">{$_('packages.loading')}</p>
  {:else if error}
    <p class="err" role="alert">{error}</p>
  {:else if payload}
    {#if !payload.reachable}
      <!-- ⚠️ Un hub auto-hébergé chez un client peut ne pas avoir Internet.
           On le DIT : sans cette phrase, les lignes sans bouton se lisent
           comme une panne du hub. Ce qui est déjà en cache reste servi. -->
      <p class="warn-inline" role="status">
        {$_('packages.offline')}
        {#if payload.error}<span class="why lp-mono">{payload.error}</span>{/if}
      </p>
    {:else if payload.version}
      <p class="hint">{$_('packages.release', { values: { version: payload.version } })}</p>
    {/if}

    <div class="list">
      {#each rows as row (row.pkg.platform)}
        <div class="pk" class:suggested={row.suggested}>
          <div class="who">
            <span class="nm">{$_(`packages.platform_${row.pkg.platform}`)}</span>
            {#if row.suggested}
              <span class="tag">{$_('packages.detected')}</span>
            {/if}
            <span class="file lp-mono">{row.pkg.asset ?? '—'}</span>
          </div>

          <div class="state">
            {#if row.state === 'cached'}
              <!-- Rien ne rentre dans ce cache sans que sa signature ait été
                   vérifiée : le dire ici, c'est dire ce que vaut le fichier. -->
              <span class="ok">
                {$_('packages.cached', {
                  values: {
                    version: row.pkg.version ?? '—',
                    size: row.pkg.size == null ? '—' : humanBytes(row.pkg.size, lang),
                  },
                })}
              </span>
            {:else if row.state === 'fetchable'}
              <span class="muted">{$_('packages.not_cached')}</span>
            {:else if row.state === 'unsigned'}
              <!-- 🔴 Dit AVANT le clic. Un bouton qui échoue à tous les coups
                   est un bouton mort — et servir ce paquet sans le vérifier
                   serait moins sûr que de renvoyer vers GitHub. -->
              <span class="warn">{$_('packages.unsigned')}</span>
            {:else if row.state === 'absent'}
              <span class="warn">{$_('packages.absent')}</span>
            {:else}
              <span class="muted">{$_('packages.offline_row')}</span>
            {/if}
          </div>

          <div class="act">
            {#if row.state === 'cached'}
              <!-- Un lien et non un `fetch` : le navigateur enregistre le
                   fichier lui-même, et le cookie de session part avec. -->
              <a class="lp-btn primary sm" href={packagesApi.fileUrl(row.pkg.platform)} download>
                {$_('packages.download')}
              </a>
            {:else if row.state === 'fetchable'}
              <button
                class="lp-btn sm"
                onclick={() => recuperer(row.pkg.platform)}
                disabled={fetching !== null}
              >
                {fetching === row.pkg.platform ? $_('packages.fetching') : $_('packages.fetch')}
              </button>
            {/if}
          </div>
        </div>
      {/each}
    </div>

    {#if fetching}
      <!-- Un bouton muet pendant qu'un paquet de 17 Mo arrive se lit comme une
           panne. On dit ce qui se passe, y compris la vérification. -->
      <p class="muted" role="status">{$_('packages.fetching_note')}</p>
    {/if}
    {#if fetchError}<p class="err" role="alert">{fetchError}</p>{/if}

    {#if payload.keep_versions > 0}
      <p class="hint">
        {$_('packages.keep', { values: { n: payload.keep_versions } })}
      </p>
    {/if}
    {#if payload.notes_url}
      <p class="hint">
        <a class="ext" href={payload.notes_url} target="_blank" rel="noopener noreferrer">
          {$_('packages.release_page')}
        </a>
      </p>
    {/if}

    <!-- ⚠️ Pas de lien App Store tant que l'app n'y est pas : un lien mort est
         pire que pas de lien. On garde la place, et on dit où on en est. -->
    <p class="hint mobile">{$_('packages.mobile_soon')}</p>
  {/if}
</Modal>

<style>
  .sub {
    font-size: 13px;
    color: var(--ep-text-secondary);
    margin: 0 0 10px;
    max-width: 70ch;
  }
  .list {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--ep-border);
    border-radius: var(--ep-radius-lg);
    overflow: hidden;
  }
  .pk {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--ep-border);
  }
  .pk:last-child {
    border-bottom: none;
  }
  /* Marqué, pas privilégié : le fond suffit à guider l'œil sans écarter les
     trois autres lignes. */
  .pk.suggested {
    background: color-mix(in srgb, var(--ep-accent) 6%, transparent);
  }
  .who {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex-wrap: wrap;
    min-width: 0;
  }
  .nm {
    font-size: 13px;
    font-weight: 600;
  }
  .tag {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--ep-accent-bright);
  }
  .file {
    font-size: 11px;
    color: var(--ep-text-dim);
    overflow-wrap: anywhere;
  }
  .state {
    font-size: 12px;
  }
  .ok {
    color: var(--ep-success);
  }
  .warn {
    color: var(--ep-warning, var(--ep-danger));
  }
  .muted {
    color: var(--ep-text-dim);
  }
  .err {
    color: var(--ep-danger);
    font-size: 12px;
  }
  .warn-inline {
    font-size: 12px;
    color: var(--ep-text-secondary);
    border-left: 2px solid var(--ep-border-strong);
    padding-left: 10px;
    margin: 0 0 10px;
  }
  .why {
    display: block;
    font-size: 11px;
    color: var(--ep-text-dim);
  }
  .hint {
    font-size: 11px;
    color: var(--ep-text-dim);
    margin: 10px 0 0;
  }
  .mobile {
    border-top: 1px solid var(--ep-border);
    padding-top: 10px;
  }
  .ext {
    color: var(--ep-accent-bright);
  }
</style>
