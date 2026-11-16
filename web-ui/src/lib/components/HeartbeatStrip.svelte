<script lang="ts">
  import type { Beat } from '$lib/heartbeat';

  interface Props {
    cells: Beat[];
    /** Ce que la bande dit, pour un lecteur d'écran : elle est une image. */
    label: string;
  }
  let { cells, label }: Props = $props();
</script>

<!--
  La bande de battement : 96 quarts d'heure, au même endroit sur chaque ligne.
  La santé se tait (vert sourd), seul le défaut prend la couleur. Les classes
  sont dans `web.css` : la fiche d'une sonde dessine la même.
-->
<div class="lp-strip" role="img" aria-label={label}>
  {#each cells as c, i (i)}<i class={c}></i>{/each}
</div>
