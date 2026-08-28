<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    tone?: 'loading' | 'empty' | 'error';
    title: string;
    body?: string;
    /** Actions (boutons, liens) — rendues sous le texte. */
    children?: Snippet;
  }
  let { tone = 'empty', title, body, children }: Props = $props();
</script>

<!--
  Chargement, vide et erreur sont trois écrans, pas trois oublis : même
  gabarit, même largeur de texte, même place pour l'action. C'est ce qui fait
  qu'un vide ressemble à une étape du produit et pas à une panne.
-->
<div class="block {tone}" role={tone === 'error' ? 'alert' : undefined} aria-busy={tone === 'loading'}>
  <div class="glyph" aria-hidden="true">
    {#if tone === 'loading'}
      <span class="spinner"></span>
    {:else if tone === 'error'}
      <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
        <circle cx="12" cy="12" r="9" />
        <path d="M12 7.5v5.5M12 16.5v.01" />
      </svg>
    {:else}
      <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
        <rect x="3" y="4" width="18" height="7" rx="2" />
        <rect x="3" y="13" width="18" height="7" rx="2" />
        <path d="M7 7.5h.01M7 16.5h.01" />
      </svg>
    {/if}
  </div>

  <h2 class="title">{title}</h2>
  {#if body}<p class="body">{body}</p>{/if}
  {#if children}<div class="actions">{@render children()}</div>{/if}
</div>

<style>
  .block {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 10px;
    padding: 48px 24px;
    border: 1px dashed var(--ep-border-strong);
    border-radius: var(--ep-radius-lg);
    background: var(--ep-glass-bg);
  }
  .error {
    border-style: solid;
    border-color: var(--ep-danger);
    background: color-mix(in srgb, var(--ep-danger) 7%, transparent);
  }
  .glyph {
    color: var(--ep-text-muted);
    display: flex;
  }
  .error .glyph {
    color: var(--ep-danger);
  }
  .title {
    font-size: 15px;
    font-weight: 700;
    color: var(--ep-text-primary);
    margin: 0;
  }
  .body {
    /* ~65 caractères : au-delà, l'œil perd la ligne suivante. */
    max-width: 52ch;
    font-size: 13px;
    line-height: 1.6;
    color: var(--ep-text-secondary);
    margin: 0;
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    justify-content: center;
    margin-top: 6px;
  }
  .spinner {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    border: 2px solid var(--ep-border-strong);
    border-top-color: var(--ep-accent);
    animation: spin 0.7s linear infinite;
    display: block;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
