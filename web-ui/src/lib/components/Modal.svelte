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
  /*
    Le dialogue est ouvert par `showModal()` — c'est vérifié plus haut. Ce qui
    le collait en haut à gauche, c'est `* { margin: 0 }` d'app.css : la feuille
    du navigateur centre `dialog:modal` avec `inset: 0; margin: auto`, et le
    reset écrasait ce `margin: auto`. On réécrit donc le centrage nous-mêmes,
    sans dépendre de la feuille UA. `height: fit-content` est indispensable :
    avec `top` et `bottom` à 0 et une hauteur automatique, la boîte s'étirerait
    sur toute la fenêtre et `margin: auto` n'aurait plus rien à répartir.
  */
  dialog {
    position: fixed;
    inset: 0;
    margin: auto;
    width: min(440px, calc(100vw - 32px));
    height: fit-content;
    max-height: calc(100dvh - 48px);
    display: flex;
    flex-direction: column;
    padding: 0;
    border: 1px solid var(--ep-border-strong);
    border-radius: var(--ep-radius-lg);
    background: var(--ep-bg-secondary);
    color: var(--ep-text-primary);
    box-shadow: 0 18px 48px rgba(0, 0, 0, 0.45);
  }
  /* `display: flex` ci-dessus l'emporterait sur le `display: none` que la
     feuille UA applique à un dialogue fermé. */
  dialog:not([open]) {
    display: none;
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
    flex-shrink: 0;
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
    overflow-y: auto;
    min-height: 0;
  }
  /* Un contenu trop grand fait défiler le corps, pas la fenêtre : l'en-tête et
     les boutons de confirmation restent visibles quoi qu'il arrive. */
  .foot {
    flex-shrink: 0;
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
