<script lang="ts">
  import type { Snippet } from 'svelte';
  import { _ } from 'svelte-i18n';

  interface Props {
    /** Identifiant de la section — sert à relier l'en-tête à son corps. */
    id: string;
    title: string;
    /** L'état de la section, à droite du titre : on le lit avant le détail. */
    summary: string;
    /**
     * Gardés pour la compatibilité des appelants. Depuis la DA « Relevé »,
     * Mon compte est à plat : AUCUNE section ne se replie, donc aucune ne peut
     * cacher un signal (appareil inconnu, clé inattendue). `pinned` ne fait
     * plus que souligner la section.
     */
    pinned?: boolean;
    open?: boolean;
    ontoggle?: () => void;
    children: Snippet;
  }
  let { id, title, summary, pinned = false, children }: Props = $props();

  const headId = $derived(`account-head-${id}`);
</script>

<!--
  🔴 Ce qui alerte ne se replie pas — et, à plat, plus rien ne se replie.
  Un appareil appairé qu'on ne reconnaît pas, une clé d'accès inattendue :
  ce sont des moyens d'accès permanents, ils doivent se voir sans geste.
-->
<section class="acc" class:pinned aria-labelledby={headId}>
  <div class="head">
    <h2 class="nm" id={headId}>{title}</h2>
    <span class="sum">{summary}</span>
  </div>
  <div class="body">{@render children()}</div>
</section>

<style>
  .acc {
    display: flex;
    flex-direction: column;
    padding-bottom: 24px;
    border-bottom: 1px solid var(--color-rule);
  }
  .acc:last-child {
    border-bottom: 0;
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 8px 16px;
    flex-wrap: wrap;
    margin-bottom: 12px;
  }
  .nm {
    margin: 0;
    font-size: 1.125rem;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  /* L'état, sur la ligne du titre. */
  .sum {
    margin-left: auto;
    font-size: 0.8125rem;
    color: var(--color-muted);
    font-variant-numeric: tabular-nums;
  }
  .acc.pinned .sum {
    color: var(--color-ink-2);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
</style>
