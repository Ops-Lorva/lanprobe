<script lang="ts">
  import type { Snippet } from 'svelte';
  import LogoMark from '$desktop/components/LogoMark.svelte';
  import { _ } from 'svelte-i18n';
  import LangTheme from '$lib/components/LangTheme.svelte';

  interface Props {
    title: string;
    lead?: string;
    /** Écran large : la colonne d'aide occupe la moitié droite. */
    aside?: Snippet;
    children: Snippet;
  }
  let { title, lead, aside, children }: Props = $props();
</script>

<!--
  Cadre commun aux écrans hors session. Il reprend la marque de l'app desktop
  (LogoMark importé, pas recopié) pour que la première page du hub soit
  reconnaissable par quelqu'un qui vient de l'application.
-->
<div class="page">
  <div class="topline">
    <div class="brand">
      <LogoMark size={26} />
      <span class="wordmark">{$_('common.app_name')}</span>
      <span class="tag">{$_('common.hub')}</span>
    </div>
    <LangTheme compact />
  </div>

  <main class="grid" class:with-aside={!!aside}>
    <section class="panel lp-card">
      <h1>{title}</h1>
      {#if lead}<p class="lead">{lead}</p>{/if}
      {@render children()}
    </section>

    {#if aside}
      <aside class="aside">{@render aside()}</aside>
    {/if}
  </main>
</div>

<style>
  .page {
    min-height: 100dvh;
    display: flex;
    flex-direction: column;
    background: var(--ep-bg-primary);
    padding: 18px;
    gap: 26px;
  }
  .topline {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 9px;
  }
  .wordmark {
    font-size: 14px;
    font-weight: 800;
    letter-spacing: 0.3px;
  }
  .tag {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 1px;
    color: var(--ep-accent-bright);
    border: 1px solid var(--ep-accent-glow);
    background: var(--ep-accent-dim);
    border-radius: 999px;
    padding: 2px 7px;
  }

  .grid {
    flex: 1;
    display: grid;
    justify-content: center;
    align-content: center;
    gap: 22px;
    width: 100%;
    max-width: 1040px;
    margin: 0 auto;
    padding-bottom: 32px;
  }
  .grid.with-aside {
    grid-template-columns: minmax(0, 400px) minmax(0, 460px);
    align-items: start;
  }
  .panel {
    padding: 26px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    width: min(400px, 100%);
  }
  .grid.with-aside .panel {
    width: auto;
  }
  h1 {
    font-size: 19px;
    font-weight: 800;
    letter-spacing: -0.02em;
    margin: 0;
  }
  .lead {
    font-size: 12.5px;
    line-height: 1.65;
    color: var(--ep-text-secondary);
    margin: -4px 0 0;
  }
  .aside {
    align-self: start;
  }

  @media (max-width: 900px) {
    .grid.with-aside {
      grid-template-columns: minmax(0, 1fr);
      max-width: 520px;
    }
  }
</style>
