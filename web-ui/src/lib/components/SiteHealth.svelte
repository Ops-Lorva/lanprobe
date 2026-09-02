<script lang="ts">
  import { _ } from 'svelte-i18n';
  import { canOperate } from '$lib/session';
  import StatusMark from './StatusMark.svelte';
  import { LIVENESS_ORDER } from '$lib/fleet-filter';
  import type { LivenessState } from '$lib/probe-liveness';

  interface Props {
    /**
     * ⚠️ Comptés sur `probeLiveness`, pas sur le statut du hub. L'en-tête d'un
     * site replié est ce qu'on lit en premier : il ne peut pas annoncer « en
     * ligne » des sondes que ses propres lignes montrent silencieuses.
     */
    counts: Record<LivenessState, number>;
    siteName: string;
    /** Somme des points en tampon du site — un site peut être « tout vert » et bourré. */
    buffered?: number;
    /** Enrôler une sonde DANS ce site. Absent = compteur seul. */
    onadd?: () => void;
    addBusy?: boolean;
  }
  let { counts, siteName, buffered = 0, onadd, addBusy = false }: Props = $props();

  const total = $derived(counts.online + counts.silent + counts.offline);

  /**
   * Ordre de lecture : ce qui va mal d'abord. Sur dix sites repliés, l'œil
   * balaie une colonne de compteurs — le rouge doit toujours occuper la même
   * position, la première, sinon il faut lire pour le trouver.
   */
  const order = LIVENESS_ORDER;

  const label = $derived(
    order
      .filter((s) => counts[s] > 0)
      .map((s) => `${counts[s]} ${$_(`status.${s}`)}`)
      .join(', '),
  );
</script>

<div class="health">
  <!--
    Barre de proportion : elle sert à repérer un site en peine SANS le déplier.
    Sur un site de trente sondes, deux hors ligne représentent une encoche
    rouge visible à un mètre ; le compteur chiffré à côté donne la valeur
    exacte, la barre donne la proportion.
  -->
  <div
    class="bar"
    role="img"
    aria-label="{$_('fleet.health_bar', { values: { name: siteName } })} : {label}"
  >
    {#each order as s (s)}
      {#if counts[s] > 0}
        <span class="seg {s}" style:flex-grow={counts[s]}></span>
      {/if}
    {/each}
  </div>

  <div class="counters">
    {#if total > 0}
      {#if counts.offline === 0 && counts.silent === 0}
        <span class="ok"><StatusMark status="online" size={10} withLabel={false} /></span>
        <span class="ok-txt">{$_('fleet.site_healthy')}</span>
      {:else}
        {#each order as s (s)}
          {#if counts[s] > 0}
            <span class="counter {s}">
              <StatusMark status={s} size={10} withLabel={false} />
              <span class="n lp-mono">{counts[s]}</span>
              <span class="cap">{$_(`status.${s}`)}</span>
            </span>
          {/if}
        {/each}
      {/if}
    {/if}

    {#if buffered > 0}
      <span class="counter buffered" title={$_('buffer.pending', { values: { n: buffered } })}>
        <span class="dot" aria-hidden="true"></span>
        <span class="cap">{$_('buffer.label')}</span>
      </span>
    {/if}

    <!--
      Le compteur est rendu dans tous les états, y compris quand le site est en
      peine : c'est lui qui ancre le « + ». Un bouton d'ajout qui se déplacerait
      selon la santé du site ne se viserait jamais deux fois au même endroit.
    -->
    <span class="tally">
      {#if total > 0}<span class="sep" aria-hidden="true">·</span>{/if}
      <span class="muted">{$_('site.probe_count', { values: { n: total } })}</span>
      <!-- Enrôler une sonde est une action sur le parc : un lecteur n'en a
           pas le droit, et le bouton n'a rien à faire sous ses yeux. -->
      {#if onadd && $canOperate}
        <button
          class="add"
          class:busy={addBusy}
          onclick={onadd}
          disabled={addBusy}
          title={$_('fleet.site_add', { values: { name: siteName } })}
          aria-label={$_('fleet.site_add', { values: { name: siteName } })}
        >
          <svg
            width="13"
            height="13"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="1.9"
            stroke-linecap="round"
            aria-hidden="true"
          >
            <path d="M8 3.5v9M3.5 8h9" />
          </svg>
        </button>
      {/if}
    </span>
  </div>
</div>

<style>
  .health {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
    flex-wrap: wrap;
  }

  .bar {
    display: flex;
    gap: 2px; /* l'espace de surface sépare les segments — pas une bordure */
    width: 108px;
    height: 6px;
    border-radius: 3px;
    overflow: hidden;
    background: var(--ep-bg-tertiary);
    flex-shrink: 0;
  }
  .seg {
    display: block;
    flex-basis: 0;
    min-width: 3px;
  }
  .seg.online {
    background: var(--ep-success);
  }
  /* Neutre, comme la pastille : l'orange affirmerait une panne que
     personne ne peut constater — la sonde s'est juste tue. */
  .seg.silent {
    background: var(--ep-text-muted);
  }
  .seg.offline {
    background: var(--ep-danger);
  }

  .counters {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 11.5px;
    flex-wrap: wrap;
  }
  .counter {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .n {
    font-weight: 700;
    color: var(--ep-text-primary);
  }
  .cap {
    color: var(--ep-text-secondary);
  }
  .counter.offline .n,
  .counter.offline .cap {
    color: var(--ep-danger);
    font-weight: 600;
  }
  .ok-txt {
    color: var(--ep-text-secondary);
  }
  .ok {
    display: inline-flex;
  }
  .muted {
    color: var(--ep-text-muted);
  }
  .buffered .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--ep-warning);
    display: inline-block;
  }
  .buffered .cap {
    color: var(--ep-warning);
    font-weight: 600;
  }

  /* Compteur et « + » forment un bloc insécable : ils se lisent comme une
     phrase — « 3 sondes, en ajouter une » — et ne se séparent pas au retour
     à la ligne des compteurs. */
  .tally {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }
  .sep {
    color: var(--ep-text-dim);
  }
  /*
    Le « + » reste plein contraste, contrairement aux actions du site qui
    s'estompent au repos : ajouter une sonde est la raison pour laquelle on
    ouvre cet écran, pas une action de maintenance qu'on cherche.
  */
  .add {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border-radius: 999px;
    border: 1px solid var(--ep-accent-glow);
    background: var(--ep-accent-dim);
    color: var(--ep-accent-bright);
    cursor: pointer;
    flex-shrink: 0;
  }
  .add:hover {
    border-color: var(--ep-accent);
    background: color-mix(in srgb, var(--ep-accent) 22%, transparent);
  }
  .add:focus-visible {
    outline: 2px solid var(--ep-accent);
    outline-offset: 2px;
  }
  /* Générer un code est un aller-retour réseau : le bouton dit qu'il travaille,
     sinon on reclique et on émet deux codes pour une seule sonde. */
  .add.busy {
    opacity: 0.55;
    cursor: progress;
  }
  /* Au doigt, 24 px se rate : la cible passe à 32 px là où il n'y a pas de
     survol pour rattraper une visée approximative. */
  @media (hover: none) {
    .add {
      width: 32px;
      height: 32px;
    }
  }
</style>
