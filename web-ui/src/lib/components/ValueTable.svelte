<script lang="ts">
  interface Props {
    columns: string[];
    rows: (string | number)[][];
    /** Plafond d'affichage : au-delà, le tableau devient illisible et lourd. */
    max?: number;
  }
  let { columns, rows, max = 400 }: Props = $props();

  const shown = $derived(rows.slice(-max).reverse());
</script>

<!--
  Le jumeau tabulaire de chaque graphique. Une infobulle ne doit jamais être le
  seul chemin vers une valeur : ici tout est lisible au clavier, copiable, et
  indépendant de la couleur.
-->
<table>
  <thead>
    <tr>
      {#each columns as c (c)}
        <th>{c}</th>
      {/each}
    </tr>
  </thead>
  <tbody>
    {#each shown as row, i (i)}
      <tr>
        {#each row as cell, j (j)}
          <td class:num={j > 0}>{cell}</td>
        {/each}
      </tr>
    {/each}
  </tbody>
</table>

<style>
  table {
    width: 100%;
    border-collapse: collapse;
    font-family: var(--ep-font-mono);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  th {
    position: sticky;
    top: 0;
    background: var(--ep-bg-secondary);
    text-align: left;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--ep-text-dim);
    padding: 6px 8px;
    border-bottom: 1px solid var(--ep-border);
  }
  td {
    padding: 4px 8px;
    color: var(--ep-text-secondary);
    border-bottom: 1px solid var(--ep-border);
    white-space: nowrap;
  }
  td.num {
    text-align: right;
    color: var(--ep-text-primary);
  }
</style>
