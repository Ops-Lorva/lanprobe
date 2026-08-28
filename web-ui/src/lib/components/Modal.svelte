<script lang="ts">
  import type { Snippet } from 'svelte';
  import { _ } from 'svelte-i18n';

  interface Props {
    open: boolean;
    title: string;
    onclose: () => void;
    children: Snippet;
    footer?: Snippet;
    /** Pour les contenus qui ne tiennent pas en 440 px (code d'enrôlement). */
    wide?: boolean;
  }
  let { open, title, onclose, children, footer, wide = false }: Props = $props();

  let el = $state<HTMLDialogElement | null>(null);

  // <dialog> natif : piège de focus, Échap et inertie de l'arrière-plan sont
  // gérés par le navigateur. Les réimplémenter à la main, c'est les rater.
  $effect(() => {
    if (!el) return;
    if (open && !el.open) el.showModal();
    if (!open && el.open) el.close();
  });
</script>

<dialog bind:this={el} class:wide onclose={onclose} oncancel={onclose} aria-label={title}>
  <div class="head">
    <h2>{title}</h2>
    <button class="lp-btn ghost sm" onclick={onclose} aria-label={$_('common.close')}>✕</button>
  </div>
  <div class="body">{@render children()}</div>
  {#if footer}<div class="foot">{@render footer()}</div>{/if}
</dialog>

<style>
  dialog {
    width: min(440px, calc(100vw - 32px));
    padding: 0;
    border: 1px solid var(--ep-border-strong);
    border-radius: var(--ep-radius-lg);
    background: var(--ep-bg-secondary);
    color: var(--ep-text-primary);
    box-shadow: 0 18px 48px rgba(0, 0, 0, 0.45);
  }
  dialog.wide {
    width: min(680px, calc(100vw - 32px));
  }
  dialog::backdrop {
    background: rgba(4, 6, 12, 0.6);
    backdrop-filter: blur(2px);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 14px 16px;
    border-bottom: 1px solid var(--ep-border);
  }
  h2 {
    font-size: 14px;
    font-weight: 700;
    margin: 0;
  }
  .body {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    font-size: 13px;
    line-height: 1.6;
    color: var(--ep-text-secondary);
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--ep-border);
    background: var(--ep-bg-tertiary);
    border-radius: 0 0 var(--ep-radius-lg) var(--ep-radius-lg);
    flex-wrap: wrap;
  }
</style>
