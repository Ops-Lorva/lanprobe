<script lang="ts">
  import type { Snippet } from 'svelte';
  import { _ } from 'svelte-i18n';

  interface Props {
    title: string;
    sub?: string;
    tone: 'loading' | 'error' | 'empty' | 'ready';
    errorText?: string;
    /** Légende — présente dès deux séries, jamais pour une seule. */
    legend?: { label: string; color: string }[];
    /** Rafraîchissement en arrière-plan : on garde le rendu précédent. */
    refreshing?: boolean;
    children: Snippet;
    table?: Snippet;
    /** Contenu libre dans l'en-tête, à gauche du bouton de tableau. */
    action?: Snippet;
  }
  let {
    title,
    sub,
    tone,
    errorText,
    legend = [],
    refreshing = false,
    children,
    table,
    action,
  }: Props = $props();

  let showTable = $state(false);
</script>

<section class="card lp-card">
  <header>
    <div class="txt">
      <h2 class="lp-title">{title}</h2>
      {#if sub}<p class="sub">{sub}</p>{/if}
    </div>

    <div class="right">
      {#if action}{@render action()}{/if}
      {#if legend.length > 1}
        <!-- La légende est le canal d'identité fiable : elle est toujours là
             dès qu'il y a deux séries, et le texte reste en encre de texte. -->
        <div class="legend">
          {#each legend as l (l.label)}
            <span class="key">
              <span class="swatch" style:background={l.color}></span>
              <span>{l.label}</span>
            </span>
          {/each}
        </div>
      {/if}
      {#if table && tone === 'ready'}
        <button class="lp-btn ghost sm" onclick={() => (showTable = !showTable)}>
          {showTable ? $_('charts.table_hide') : $_('charts.table_show')}
        </button>
      {/if}
    </div>
  </header>

  <div class="body" class:dim={refreshing}>
    {#if tone === 'loading'}
      <div class="msg"><span class="spinner"></span>{$_('charts.loading')}</div>
    {:else if tone === 'error'}
      <p class="msg err" role="alert">{errorText || $_('charts.error')}</p>
    {:else if tone === 'empty'}
      <p class="msg">
        {$_('charts.empty')}
        <span class="hint">{$_('charts.empty_hint')}</span>
      </p>
    {:else}
      {@render children()}
      {#if showTable && table}
        <div class="table">{@render table()}</div>
      {/if}
    {/if}
  </div>
</section>

<style>
  .card {
    padding: 14px 14px 12px;
    /* La carte grandit avec son contenu : une hauteur figée finirait par
       rogner la bande d'axe et créer un mini-défilement interne. */
  }
  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
    margin-bottom: 10px;
  }
  .sub {
    font-size: 11.5px;
    color: var(--ep-text-muted);
    margin: 3px 0 0;
    max-width: 60ch;
    line-height: 1.5;
  }
  .right {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .legend {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
  }
  .key {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    color: var(--ep-text-secondary);
  }
  .swatch {
    width: 10px;
    height: 3px;
    border-radius: 2px;
  }

  .body {
    transition: opacity 0.15s;
  }
  .body.dim {
    opacity: 0.55;
  }

  .msg {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    min-height: 96px;
    font-size: 12.5px;
    color: var(--ep-text-secondary);
    margin: 0;
  }
  .msg.err {
    color: var(--ep-danger);
  }
  .hint {
    color: var(--ep-text-muted);
    font-size: 11.5px;
  }

  .table {
    margin-top: 12px;
    max-height: 260px;
    overflow: auto;
    border-top: 1px solid var(--ep-border);
  }

  .spinner {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 2px solid var(--ep-border-strong);
    border-top-color: var(--ep-accent);
    animation: spin 0.7s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
