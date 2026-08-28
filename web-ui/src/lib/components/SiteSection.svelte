<script lang="ts">
  import { _ } from 'svelte-i18n';
  import ProbeRow from './ProbeRow.svelte';
  import SiteHealth from './SiteHealth.svelte';
  import StateBlock from './StateBlock.svelte';
  import PendingEnrollRow from './PendingEnrollRow.svelte';
  import type { Probe, ProbeStatus } from '$lib/api';
  import type { PendingRow } from '$lib/enroll';

  interface Props {
    siteId: string;
    siteName: string;
    probes: Probe[];
    /** Emplacements réservés de ce site : codes d'enrôlement encore ouverts. */
    pending: PendingRow[];
    now: number;
    expanded: boolean;
    ontoggle: () => void;
    onrename: () => void;
    onfocusSite: () => void;
    /** Crée un code d'enrôlement POUR CE SITE : le site n'est jamais à choisir. */
    onadd: () => void;
    /** Génération en cours pour ce site : le « + » ne doit pas se cliquer deux fois. */
    addBusy?: boolean;
    /** Échec de la génération, montré à l'endroit du clic. */
    addError?: string;
    onregenerate: (row: PendingRow) => void;
    onhidePending: (row: PendingRow) => void;
    /** Clé de la ligne en attente dont la génération tourne. */
    pendingBusyKey?: string | null;
    /** Vrai quand un filtre est actif : le vide « site sans sonde » ne s'applique plus. */
    filtered: boolean;
  }
  let {
    siteId,
    siteName,
    probes,
    pending,
    now,
    expanded,
    ontoggle,
    onrename,
    onfocusSite,
    onadd,
    addBusy = false,
    addError = '',
    onregenerate,
    onhidePending,
    pendingBusyKey = null,
    filtered,
  }: Props = $props();

  const counts = $derived.by(() => {
    const c: Record<ProbeStatus, number> = { online: 0, stale: 0, offline: 0 };
    for (const p of probes) c[p.status]++;
    return c;
  });
  const buffered = $derived(probes.reduce((sum, p) => sum + (p.buffered_points || 0), 0));
  const degraded = $derived(counts.offline > 0);
  const bodyId = $derived(`site-body-${siteId}`);
</script>

<section class="site" class:degraded>
  <!--
    L'en-tête porte tout ce qu'il faut pour décider s'il vaut la peine de
    déplier : barre de proportion, compteurs par statut, alerte de tampon. Sans
    ça on ne déplie que les sites qu'on soupçonne déjà, et on rate les autres.
  -->
  <div class="head">
    <button
      class="toggle"
      onclick={ontoggle}
      aria-expanded={expanded}
      aria-controls={bodyId}
      aria-label={expanded
        ? $_('fleet.site_collapse', { values: { name: siteName } })
        : $_('fleet.site_expand', { values: { name: siteName } })}
    >
      <span class="chev" class:open={expanded} aria-hidden="true">
        <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="6,3 11,8 6,13" />
        </svg>
      </span>
      <span class="nm">{siteName}</span>
    </button>

    <SiteHealth {counts} siteName={siteName} {buffered} {onadd} addBusy={addBusy} />

    <div class="acts">
      <button
        class="lp-btn ghost sm"
        onclick={onfocusSite}
        title={$_('fleet.site_open')}
        aria-label={$_('fleet.site_open')}
      >
        <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M2 3h12l-4.6 5.4v4.2L6.6 14V8.4z" />
        </svg>
      </button>
      <button
        class="lp-btn ghost sm"
        onclick={onrename}
        title={$_('fleet.site_rename')}
        aria-label={$_('fleet.site_menu', { values: { name: siteName } })}
      >
        <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M11.2 2.3l2.5 2.5L5.5 13H3v-2.5z" />
        </svg>
      </button>
    </div>
  </div>

  {#if expanded}
    <div class="body" id={bodyId}>
      <!--
        Les attentes passent AVANT les sondes : on vient de cliquer sur « + »,
        le code est ce qu'on cherche des yeux. Le chercher au bas d'une liste de
        trente lignes serait le rater.
      -->
      {#if addError}
        <p class="add-err" role="alert">{addError}</p>
      {/if}
      {#each pending as row (row.key)}
        <PendingEnrollRow
          {row}
          busy={pendingBusyKey === row.key}
          onregenerate={() => onregenerate(row)}
          onhide={() => onhidePending(row)}
        />
      {/each}

      {#if probes.length === 0 && pending.length === 0 && !filtered}
        <div class="empty">
          <StateBlock
            title={$_('fleet.empty_site_title')}
            body={$_('fleet.empty_site_body', { values: { name: siteName } })}
          >
            <button class="lp-btn primary" onclick={onadd}>
              {$_('fleet.empty_site_cta')}
            </button>
          </StateBlock>
        </div>
      {:else}
        {#each probes as p (p.probe_id)}
          <ProbeRow probe={p} {now} />
        {/each}
      {/if}
    </div>
  {/if}
</section>

<style>
  .site {
    background: var(--ep-bg-secondary);
    border: 1px solid var(--ep-border);
    border-radius: var(--ep-radius-lg);
    overflow: hidden;
  }
  /* Un site qui porte une sonde hors ligne se signale par son cadre : repérable
     dans une pile de dix sites repliés, sans avoir à comparer des compteurs. */
  .site.degraded {
    border-color: color-mix(in srgb, var(--ep-danger) 45%, var(--ep-border));
  }

  .head {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 10px 10px 10px 12px;
    background: var(--ep-glass-bg);
    border-bottom: 1px solid transparent;
    flex-wrap: wrap;
  }
  .site.degraded .head {
    background: color-mix(in srgb, var(--ep-danger) 5%, transparent);
  }

  .toggle {
    display: flex;
    align-items: center;
    gap: 8px;
    background: none;
    border: none;
    padding: 2px 0;
    cursor: pointer;
    color: var(--ep-text-primary);
    font-family: var(--ep-font-sans);
    font-size: 13.5px;
    font-weight: 700;
    letter-spacing: -0.01em;
    min-width: 0;
    flex: 1 1 180px;
    text-align: left;
  }
  .chev {
    display: flex;
    color: var(--ep-text-muted);
    transition: transform 0.14s;
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .nm {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .acts {
    display: flex;
    gap: 2px;
    margin-left: auto;
  }
  /* Les actions du site ne sont pas des décisions de balayage : discrètes au
     repos, pleines au survol et au focus clavier. Toujours visibles au tactile,
     où il n'y a pas de survol. */
  .acts :global(.lp-btn) {
    opacity: 0.5;
  }
  .head:hover .acts :global(.lp-btn),
  .acts :global(.lp-btn:focus-visible) {
    opacity: 1;
  }
  @media (hover: none) {
    .acts :global(.lp-btn) {
      opacity: 1;
    }
  }

  .body {
    border-top: 1px solid var(--ep-border);
  }
  .empty {
    padding: 12px;
  }
  /* L'échec est rendu dans le site où le clic a eu lieu : un bandeau global
     obligerait à retrouver lequel des dix sites a refusé. */
  .add-err {
    margin: 10px 12px;
    font-size: 11.5px;
    color: var(--ep-danger);
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
    overflow-wrap: anywhere;
  }
</style>
