<script lang="ts">
  import type { Snippet } from 'svelte';
  import { _ } from 'svelte-i18n';

  interface Props {
    /** Identifiant de la section — sert à relier l'en-tête à son corps. */
    id: string;
    title: string;
    /**
     * Ce que la section dit FERMÉE. Un accordéon muet oblige à tout ouvrir,
     * donc ne fait rien gagner sur la pile qu'il remplace.
     */
    summary: string;
    /**
     * 🔴 La section porte un signal : elle ne se replie pas, et n'a donc pas
     * de bouton. Un bouton grisé se cliquerait trois fois avant qu'on
     * comprenne.
     */
    pinned: boolean;
    open: boolean;
    ontoggle: () => void;
    children: Snippet;
  }
  let { id, title, summary, pinned, open, ontoggle, children }: Props = $props();

  const bodyId = $derived(`account-body-${id}`);
</script>

<!--
  🔴 Ce qui alerte ne se replie pas. Un appareil appairé qu'on ne reconnaît
  pas, une clé d'accès inattendue : ce sont des moyens d'accès permanents, et
  personne ne déplie une section qu'il ne soupçonne pas déjà. Un accordéon qui
  cache un signal est pire que la pile qu'il remplace.
-->
<section class="acc lp-card" class:pinned>
  {#if pinned}
    <div class="head">
      <h2 class="lp-title nm">{title}</h2>
      <span class="sum">{summary}</span>
      <!-- Dire POURQUOI elle n'a pas de bouton : sans cette mention, une
           section qui ne se replie pas se lit comme une panne de l'interface,
           pas comme le signal qu'elle porte. -->
      <span class="always">{$_('account.always_open')}</span>
    </div>
  {:else}
    <button
      class="head toggle"
      onclick={ontoggle}
      aria-expanded={open}
      aria-controls={bodyId}
      aria-label={open
        ? $_('account.section_collapse', { values: { name: title } })
        : $_('account.section_expand', { values: { name: title } })}
    >
      <span class="chev" class:open aria-hidden="true">
        <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="6,3 11,8 6,13" />
        </svg>
      </span>
      <h2 class="lp-title nm">{title}</h2>
      <span class="sum">{summary}</span>
    </button>
  {/if}

  {#if pinned || open}
    <div class="body" id={bodyId}>{@render children()}</div>
  {/if}
</section>

<style>
  .acc {
    display: flex;
    flex-direction: column;
  }
  /* La section épinglée se distingue au trait, pas à la couleur : elle n'est
     pas en faute, elle porte simplement quelque chose qui doit se voir. */
  .acc.pinned {
    border-color: var(--ep-border-strong);
  }

  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    width: 100%;
    padding: 14px 18px;
    text-align: left;
  }
  .toggle {
    border: none;
    background: transparent;
    color: inherit;
    font: inherit;
    cursor: pointer;
    border-radius: var(--ep-radius-lg);
  }
  .toggle:hover .nm {
    color: var(--ep-accent);
  }
  .toggle:focus-visible {
    outline: 2px solid var(--ep-accent);
    outline-offset: -2px;
  }

  .chev {
    display: inline-flex;
    color: var(--ep-text-dim);
    transition: transform 0.12s ease;
  }
  .chev.open {
    transform: rotate(90deg);
  }

  .nm {
    margin: 0;
  }
  /* L'état, à droite du titre et sur la même ligne : c'est lui qu'on lit pour
     décider s'il vaut la peine de déplier. */
  .sum {
    margin-left: auto;
    font-size: 12px;
    color: var(--ep-text-secondary);
    font-variant-numeric: tabular-nums;
  }
  .always {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--ep-text-dim);
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 0 18px 16px;
    border-top: 1px solid var(--ep-border);
    padding-top: 14px;
  }
</style>
