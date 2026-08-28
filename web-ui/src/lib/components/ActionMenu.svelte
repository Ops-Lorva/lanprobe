<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icons from '$desktop/components/Icons.svelte';

  interface Props {
    /** Libellé du déclencheur : lu par les lecteurs d'écran, affiché en survol. */
    label: string;
    /** Reçoit la fonction de fermeture : un item qui agit referme le menu. */
    children: Snippet<[() => void]>;
  }
  let { label, children }: Props = $props();

  let open = $state(false);
  let root = $state<HTMLDivElement | null>(null);
  let trigger = $state<HTMLButtonElement | null>(null);
  let panel = $state<HTMLDivElement | null>(null);

  function items(): HTMLElement[] {
    return panel ? Array.from(panel.querySelectorAll<HTMLElement>('[role="menuitem"]')) : [];
  }

  function close(refocus = true) {
    if (!open) return;
    open = false;
    // Rendre le focus au déclencheur : sans ça, une fermeture au clavier
    // renvoie le focus au début du document et la place est perdue.
    if (refocus) trigger?.focus();
  }

  function toggle() {
    open = !open;
  }

  // Le premier item prend le focus à l'ouverture : le menu est alors utilisable
  // entièrement au clavier, ce qui compte sur un parc qu'on parcourt vite.
  $effect(() => {
    if (open) items()[0]?.focus();
  });

  function onWindowPointer(e: PointerEvent) {
    if (!open || !root) return;
    if (!root.contains(e.target as Node)) close(false);
  }

  function onWindowKey(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    }
  }

  function onPanelKey(e: KeyboardEvent) {
    const list = items();
    if (list.length === 0) return;
    const i = list.indexOf(document.activeElement as HTMLElement);
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      list[(i + 1 + list.length) % list.length].focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      list[(i - 1 + list.length) % list.length].focus();
    } else if (e.key === 'Home') {
      e.preventDefault();
      list[0].focus();
    } else if (e.key === 'End') {
      e.preventDefault();
      list[list.length - 1].focus();
    } else if (e.key === 'Tab') {
      // Tab hors du menu = intention de partir : on ferme sans reprendre le focus.
      close(false);
    }
  }
</script>

<svelte:window onpointerdown={onWindowPointer} onkeydown={onWindowKey} />

<div class="menu-root" bind:this={root}>
  <button
    class="lp-btn sm gear"
    bind:this={trigger}
    onclick={toggle}
    aria-haspopup="menu"
    aria-expanded={open}
    aria-label={label}
    title={label}
  >
    <Icons name="gear" size={15} />
    <svg
      class="caret"
      width="9"
      height="9"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <polyline points="3,6 8,11 13,6" />
    </svg>
  </button>

  {#if open}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="panel"
      role="menu"
      aria-label={label}
      bind:this={panel}
      onkeydown={onPanelKey}
      tabindex="-1"
    >
      {@render children(() => close(false))}
    </div>
  {/if}
</div>

<style>
  .menu-root {
    position: relative;
    display: inline-block;
  }
  .gear {
    gap: 4px;
    padding: 5px 8px;
  }
  .caret {
    opacity: 0.7;
  }

  /*
    Ancré à droite du déclencheur : le menu s'ouvre vers l'intérieur de la page
    et ne peut donc pas déborder du côté où il n'y a plus de place. Largeur
    plafonnée à la fenêtre moins les marges — un menu ne fait jamais défiler la
    page en largeur.
  */
  .panel {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 40;
    width: min(380px, calc(100vw - 32px));
    max-height: min(70vh, 620px);
    overflow-y: auto;
    padding: 6px;
    background: var(--ep-bg-secondary);
    border: 1px solid var(--ep-border-strong);
    border-radius: var(--ep-radius-lg);
    box-shadow: 0 16px 40px rgba(0, 0, 0, 0.4);
  }
</style>
