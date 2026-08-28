<script lang="ts">
  import { _, locale } from 'svelte-i18n';
  import { compactNumber } from '$lib/time';

  interface Props {
    sites: number;
    total: number;
    online: number;
    stale: number;
    offline: number;
    buffered: number;
  }
  let { sites, total, online, stale, offline, buffered }: Props = $props();

  const lang = $derived($locale ?? 'en');
</script>

<!--
  Reprise littérale du gabarit de tuile de l'app desktop (`StatRail.svelte`) :
  même hauteur, même étiquette 9 px capitalisée, même valeur 18 px. Seules les
  grandeurs changent. Un utilisateur du desktop retrouve le même bandeau.

  Les compteurs à zéro restent en gris : un « 0 hors ligne » en rouge apprend à
  l'œil à ignorer le rouge.
-->
<div class="rail">
  <div class="tile">
    <div class="lp-label">{$_('fleet.rail_sites')}</div>
    <div class="val">{sites}</div>
  </div>
  <div class="tile">
    <div class="lp-label">{$_('fleet.rail_total')}</div>
    <div class="val">{total}</div>
  </div>
  <div class="tile">
    <div class="lp-label">{$_('fleet.rail_online')}</div>
    <div class="val" class:ok={online > 0}>{online}</div>
  </div>
  <div class="tile">
    <div class="lp-label">{$_('fleet.rail_stale')}</div>
    <div class="val" class:warn={stale > 0}>{stale}</div>
  </div>
  <div class="tile" class:alert={offline > 0}>
    <div class="lp-label">{$_('fleet.rail_offline')}</div>
    <div class="val" class:err={offline > 0}>{offline}</div>
  </div>
  <div class="tile" class:alert={buffered > 0}>
    <div class="lp-label">{$_('fleet.rail_buffered')}</div>
    <div class="val" class:warn={buffered > 0}>{compactNumber(buffered, lang)}</div>
  </div>
</div>

<style>
  .rail {
    display: grid;
    grid-template-columns: repeat(6, 1fr);
    gap: 6px;
  }
  .tile {
    background: var(--ep-bg-secondary);
    border: 1px solid var(--ep-border);
    border-radius: var(--ep-radius-md);
    padding: 10px 12px;
  }
  /* Une tuile qui compte quelque chose d'anormal se démarque par son cadre,
     pas seulement par la couleur du chiffre. */
  .tile.alert {
    border-color: color-mix(in srgb, var(--ep-danger) 45%, var(--ep-border));
  }
  .val {
    font-size: 18px;
    font-weight: 700;
    color: var(--ep-text-primary);
    line-height: 1.25;
  }
  .val.ok {
    color: var(--ep-success);
  }
  .val.warn {
    color: var(--ep-warning);
  }
  .val.err {
    color: var(--ep-danger);
  }

  @media (max-width: 860px) {
    .rail {
      grid-template-columns: repeat(3, 1fr);
    }
  }
  @media (max-width: 460px) {
    .rail {
      grid-template-columns: repeat(2, 1fr);
    }
  }
</style>
