<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { _ } from 'svelte-i18n';
  import CopyLine from './CopyLine.svelte';
  import type { PendingRow } from '$lib/enroll';

  interface Props {
    row: PendingRow;
    busy?: boolean;
    error?: string;
    onregenerate: () => void;
    onhide: () => void;
    /**
     * Ouvre la liste des paquets de la sonde.
     *
     * 🔴 C'est ICI que le lien manquait : le hub donne un code d'enrôlement, et
     * il fallait aller chercher le binaire ailleurs. Un seul bouton, une seule
     * fenêtre — quatre boutons par plateforme noieraient le geste principal de
     * cet écran, qui reste l'enrôlement.
     */
    ondownload: () => void;
  }
  let { row, busy = false, error = '', onregenerate, onhide, ondownload }: Props = $props();

  /** Ré-enrôlement = réparation d'une sonde existante. Ce n'est pas le même
      geste qu'ajouter une machine, et ça ne doit pas s'y confondre. */
  const repair = $derived(row.probe_id != null);

  let tick = $state(Date.now());
  let timer: ReturnType<typeof setInterval> | null = null;
  onMount(() => {
    timer = setInterval(() => (tick = Date.now()), 1000);
  });
  onDestroy(() => {
    if (timer) clearInterval(timer);
  });

  const remaining = $derived(row.expires_at * 1000 - tick);
  const expired = $derived(remaining <= 0);
  const countdown = $derived.by(() => {
    const s = Math.max(0, Math.floor(remaining / 1000));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  });

  /**
   * Regroupement par quatre. Le code se lit à voix haute au téléphone : chasse
   * fixe, paquets courts, et zéro barré demandé à la fonte pour que 0/O et 1/l
   * ne se confondent pas. C'est la fonte qui porte la lisibilité, pas la taille
   * — le corps reste celui du texte courant.
   */
  const groups = $derived.by(() => {
    const raw = (row.code ?? '').replace(/[^A-Za-z0-9]/g, '').toUpperCase();
    return raw.match(/.{1,4}/g) ?? [raw];
  });

  /** Cible de repli quand le presse-papiers est refusé (hub servi en HTTP). */
  let codeEl = $state<HTMLElement | null>(null);
</script>

<!--
  Une ligne du tableau, au gabarit exact de ProbeRow : même grille, même hauteur,
  même rythme vertical. Trois enrôlements en cours ne déforment donc plus la
  liste, et l'œil ne balaie qu'une seule mise en page.

  Ce qui reste : le code et le chrono. Le distinguo « place réservée, pas
  équipement » se joue sur le fond et la bordure gauche en pointillés, plus sur
  la taille.
-->
<div
  class="p-row"
  class:repair
  class:expired
  class:gone={!row.code}
  role="group"
  aria-label={repair
    ? $_('pending.aria_reenroll', { values: { name: row.probe ?? '', code: row.code ?? '' } })
    : $_('pending.aria_new', { values: { code: row.code ?? '' } })}
>
  <span class="c-code">
    {#if row.code}
      <span class="p-code lp-mono" bind:this={codeEl}>
        {#each groups as g, i (i)}
          {#if i > 0}<span class="sep" aria-hidden="true">–</span>{/if}<span>{g}</span>
        {/each}
      </span>
      <CopyLine value={row.code} shell={false} hideValue compact selectTarget={codeEl} />
    {:else}
      <span class="lost">{$_('pending.lost_title')}</span>
    {/if}
    {#if repair && row.probe}
      <span class="for">{$_('pending.for_probe', { values: { name: row.probe } })}</span>
    {/if}
  </span>

  <span class="c-badge">
    <span class="badge">
      <span class="pip" aria-hidden="true"></span>
      {repair ? $_('pending.badge_reenroll') : $_('pending.badge_new')}
    </span>
  </span>

  <span class="c-ttl lp-mono" class:dead={expired}>
    {expired ? $_('pending.expired') : countdown}
  </span>

  <span class="c-acts">
    <button
      class="ib"
      onclick={onregenerate}
      disabled={busy}
      title={$_('code.regenerate')}
      aria-label={$_('code.regenerate')}
    >
      <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
        <path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9" />
        <polyline points="13.5,2 13.5,5 10.5,5" />
      </svg>
    </button>
    <!-- ⚠️ N'appelle RIEN au chargement : le code d'enrôlement s'affiche sur un
         hub sans Internet comme sur les autres. C'est la fenêtre qui sort, et
         seulement quand on l'ouvre. -->
    <button
      class="ib"
      onclick={ondownload}
      title={$_('packages.open_hint')}
      aria-label={$_('packages.open')}
    >
      <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
        <path d="M8 2v8" />
        <polyline points="4.5,7 8,10.5 11.5,7" />
        <path d="M3 13h10" />
      </svg>
    </button>
    <!-- « Masquer » et non « Annuler » : le hub garde le code valable, cette
         action ne fait que retirer la ligne de l'écran. -->
    <button class="ib" onclick={onhide} title={$_('pending.hide_hint')} aria-label={$_('pending.hide')}>
      <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
        <path d="M3.5 3.5l9 9M12.5 3.5l-9 9" />
      </svg>
    </button>
  </span>
</div>

{#if error}<p class="err" role="alert">{error}</p>{/if}

<style>
  /* Grille identique à ProbeRow.svelte — les styles Svelte sont scopés, la
     définition ne peut pas être partagée. Toute retouche ici se reporte là-bas. */
  .p-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 118px 120px 92px 84px 78px 18px;
    align-items: center;
    gap: 10px;
    /* 6 + 22 (gabarit des boutons) + 5 + 1 de bordure = 34 px, la hauteur
       exacte d'une ligne de sonde. Toute retouche de ces valeurs se vérifie
       à la mesure, pas à l'œil. */
    padding: 6px 12px 5px 9px;
    border-left: 3px dashed var(--ep-accent);
    border-bottom: 1px solid var(--ep-border);
    background: color-mix(in srgb, var(--ep-accent) 5%, transparent);
    font-size: 12px;
    min-height: 33px;
  }
  .p-row:last-child {
    border-bottom: none;
  }
  /* Réparer une sonde compromise n'est pas ajouter une machine : la teinte
     change avec l'opération, pas seulement le libellé. */
  .p-row.repair {
    border-left-color: var(--ep-warning);
    background: color-mix(in srgb, var(--ep-warning) 6%, transparent);
  }
  .p-row.expired,
  .p-row.gone {
    border-left-color: var(--ep-border-strong);
    background: var(--ep-bg-tertiary);
  }

  .c-code {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  /* Corps du texte courant, pas un affichage géant : c'est la chasse fixe et le
     groupement par quatre qui rendent le code dictable, pas sa taille. */
  .p-code {
    font-size: 13px;
    font-weight: 700;
    line-height: 1.2;
    color: var(--ep-accent-bright);
    letter-spacing: 1.5px;
    white-space: nowrap;
    /* Le code ne se comprime jamais : c'est le contexte autour qui cède. */
    flex-shrink: 0;
    font-variant-numeric: slashed-zero tabular-nums;
    font-feature-settings: 'zero' 1, 'ss01' 1;
  }
  .repair .p-code {
    color: var(--ep-warning);
  }
  .sep {
    color: var(--ep-text-dim);
    font-weight: 400;
    letter-spacing: 0;
  }
  .lost {
    font-size: 12px;
    color: var(--ep-text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .for {
    font-size: 11px;
    color: var(--ep-text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  /* Conteneur flex plutôt qu'un span : la boîte de ligne d'un span ajoutait
     2,5 px sous la pastille et faisait dépasser la rangée basse. */
  .c-badge {
    display: flex;
    align-items: center;
    min-width: 0;
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 9px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    /* Interligne verrouillé : sans lui, la pastille prend l'interligne normal
       de la fonte et fait grandir la rangée basse en présentation empilée. */
    line-height: 1;
    color: var(--ep-accent-bright);
    border: 1px solid var(--ep-accent-glow);
    background: var(--ep-accent-dim);
    border-radius: 999px;
    padding: 3px 7px;
    white-space: nowrap;
  }
  .repair .badge {
    color: var(--ep-warning);
    border-color: color-mix(in srgb, var(--ep-warning) 45%, var(--ep-border));
    background: color-mix(in srgb, var(--ep-warning) 12%, transparent);
  }
  .gone .badge,
  .expired .badge {
    color: var(--ep-text-muted);
    border-color: var(--ep-border-strong);
    background: transparent;
  }

  .c-ttl {
    font-size: 11px;
    font-weight: 700;
    color: var(--ep-warning);
  }
  .c-ttl.dead {
    color: var(--ep-text-muted);
  }

  .c-acts {
    grid-column: 4 / -1;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 2px;
  }
  /* Boutons au gabarit de la ligne : un `.lp-btn sm` fait 28 px et ferait
     grossir la ligne de 40 % à lui seul. */
  .ib {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 1px solid transparent;
    border-radius: var(--ep-radius-sm);
    background: transparent;
    color: var(--ep-text-muted);
    cursor: pointer;
    flex-shrink: 0;
  }
  .ib:hover:not(:disabled) {
    color: var(--ep-accent-bright);
    border-color: var(--ep-border-strong);
    background: var(--ep-bg-tertiary);
  }
  .ib:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .err {
    margin: 0;
    padding: 6px 12px 6px 9px;
    font-size: 11.5px;
    color: var(--ep-danger);
    border-left: 3px solid var(--ep-danger);
    border-bottom: 1px solid var(--ep-border);
    overflow-wrap: anywhere;
  }

  /* Les colonnes tombent dans le même ordre que ProbeRow. */
  @media (max-width: 1080px) {
    .p-row {
      grid-template-columns: minmax(0, 1fr) 118px 120px 84px 78px 18px;
    }
  }
  @media (max-width: 900px) {
    .p-row {
      grid-template-columns: minmax(0, 1fr) 118px 110px 78px 18px;
    }
  }

  /*
    Sous 640 px ProbeRow s'empile en deux rangées ; la ligne en attente fait de
    même. Le code garde sa taille et reste sur une seule ligne : c'est là qu'on
    le lit, un téléphone à la main dans une baie.
  */
  @media (max-width: 640px) {
    /*
      Deux rangées comme ProbeRow. Les deux commandes de 22 px tiennent sur la
      MÊME rangée : c'est ce qui garde la ligne à 56 px, la hauteur exacte d'une
      ligne de sonde, sans rapetisser les cibles tactiles.
      Rangée haute : ce qu'on lit et ce sur quoi on appuie.
      Rangée basse : les deux étiquettes — temps restant, nature de la ligne.
    */
    .p-row {
      grid-template-columns: minmax(0, 1fr) auto;
      grid-template-areas:
        'code acts'
        'meta badge';
      row-gap: 4px;
      padding: 6px 12px 6px 9px;
      min-height: 0;
    }
    .c-code {
      grid-area: code;
    }
    .c-acts {
      grid-area: acts;
    }
    .c-ttl {
      grid-area: meta;
    }
    .c-badge {
      grid-area: badge;
      justify-self: end;
    }
  }
</style>
