<script lang="ts">
  import { _ } from 'svelte-i18n';
  import { canOperate } from '$lib/session';
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
    /** Ouvre les alertes DU SITE : c'est ici qu'on décide qui réveille qui. */
    onalerts: () => void;
    /** Site retiré du parc, mais rien n'a été supprimé. */
    archived?: boolean;
    onarchive: () => void;
    onfocusSite: () => void;
    /** Ouvre le rapport SLA du site — la sélection des sondes se fait là. */
    onexportSla: () => void;
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
    onalerts,
    archived = false,
    onarchive,
    onfocusSite,
    onadd,
    addBusy = false,
    addError = '',
    onregenerate,
    onhidePending,
    pendingBusyKey = null,
    filtered,
    onexportSla,
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
      {#if archived}
        <!-- Un site archivé n'apparaît que si on a demandé à les voir : le
             dire sur la ligne évite de croire qu'il est encore actif. -->
        <span class="arch-pill">{$_('site.archived_badge')}</span>
      {/if}
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
      <!-- ⚠️ Masquage ET vérification serveur, pas l'un OU l'autre. Le
           masquage évite la tentation et le clic qui échoue ; c'est la
           vérification qui évite l'accès, parce qu'il suffit d'appeler la
           route à la main. Voir § 18 du contrat.

           Le rapport SLA, lui, reste visible pour un lecteur : le produire
           ne change rien au parc. -->
      <!-- Le rapport se demande depuis le SITE : c'est l'unité qu'on remet à
           un client, pas la sonde. Une sonde seule reste exportable depuis sa
           fiche. -->
      <button
        class="lp-btn ghost sm"
        onclick={onexportSla}
        title={$_('sla.export')}
        aria-label={$_('sla.export')}
      >
        <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M8 2v8m0 0L5 7m3 3l3-3" />
          <path d="M2.5 11v2.5h11V11" />
        </svg>
      </button>
      {#if $canOperate}
      <!-- Les alertes se règlent là où l'on pense au site, pas dans un écran
           de réglages qu'on ouvre une fois par trimestre. La question « ce
           site doit-il réveiller quelqu'un ? » se pose en regardant ses
           sondes, donc ici. -->
      <button
        class="lp-btn ghost sm"
        onclick={onalerts}
        title={$_('notify.site_alerts')}
        aria-label={$_('notify.site_alerts_for', { values: { name: siteName } })}
      >
        <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M8 2a3.5 3.5 0 0 0-3.5 3.5c0 3-1.2 4-1.2 4h9.4s-1.2-1-1.2-4A3.5 3.5 0 0 0 8 2z" />
          <path d="M6.8 11.6a1.4 1.4 0 0 0 2.4 0" />
        </svg>
      </button>
      <button
        class="lp-btn ghost sm"
        onclick={onarchive}
        title={archived ? $_('site.unarchive') : $_('site.archive')}
        aria-label={archived ? $_('site.unarchive') : $_('site.archive')}
      >
        <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M2 4.5h12v2H2z" />
          <path d="M3 6.5v6h10v-6" />
          <path d="M6.5 9h3" />
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
      {/if}
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
        <!-- En-tête de colonnes répété à chaque site : posé une seule fois en
             haut du parc, il finissait hors de vue dès qu'on faisait défiler,
             et il fallait le chercher pour relire une colonne. Décoratif pour
             les lecteurs d'écran, chaque ligne portant déjà ses libellés. -->
        <div class="cols" aria-hidden="true">
          <span>{$_('fleet.col_probe')}</span>
          <span>{$_('fleet.col_status')}</span>
          <span>{$_('fleet.col_last_seen')}</span>
          <span class="c-pubip">{$_('fleet.col_public_ip')}</span>
          <span class="c-platform">{$_('fleet.col_platform')}</span>
          <span class="c-version">{$_('fleet.col_version')}</span>
          <span>{$_('fleet.col_buffer')}</span>
          <span></span>
        </div>
        {#each probes as p (p.probe_id)}
          <ProbeRow probe={p} {now} />
        {/each}
      {/if}
    </div>
  {/if}
</section>

<style>
  /* ⚠️ Les points de rupture doivent suivre EXACTEMENT ceux de `ProbeRow` :
     l'en-tête et les lignes sont deux grilles indépendantes, et une colonne
     de plus d'un côté décale tous les intitulés. Ils avaient été aplatis en
     trois règles sans `@media` — la plus étroite gagnait à toute largeur, et
     l'en-tête ne correspondait plus à rien. */
  .arch-pill {
    margin-left: 8px;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.4px;
    text-transform: uppercase;
    padding: 2px 7px;
    border-radius: 999px;
    border: 1px solid var(--lp-border);
    color: var(--lp-muted);
    vertical-align: middle;
  }
  .cols {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 118px 120px 118px 92px 84px 78px 18px;
    gap: 10px;
    padding: 0 12px 6px 12px;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--ep-text-dim);
  }
  @media (max-width: 1080px) {
    .cols {
      grid-template-columns: minmax(0, 1fr) 118px 120px 118px 84px 78px 18px;
    }
    .cols .c-platform { display: none; }
  }
  @media (max-width: 900px) {
    .cols {
      grid-template-columns: minmax(0, 1fr) 118px 110px 78px 18px;
    }
    .cols .c-version,
    .cols .c-pubip { display: none; }
  }
  @media (max-width: 640px) {
    /* Les lignes passent en présentation empilée : un en-tête de colonnes
       n'a plus rien à titrer. */
    .cols { display: none; }
  }
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
