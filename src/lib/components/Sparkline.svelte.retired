<script lang="ts">
  interface Props {
    values: number[];
    color?: string;
    width?: number;
    height?: number;
  }
  let { values, color = 'var(--ep-accent)', width = 40, height = 10 }: Props = $props();

  const max = $derived(Math.max(1, ...values));
  const barWidth = $derived(values.length > 0 ? Math.max(1, Math.floor((width - (values.length - 1)) / values.length)) : 0);
</script>

<svg {width} {height} viewBox="0 0 {width} {height}" aria-hidden="true" class="spark">
  {#each values as v, i}
    {@const h = v > 0 ? Math.max(1, Math.round((v / max) * height)) : 1}
    {@const x = i * (barWidth + 1)}
    {@const y = height - h}
    <rect {x} {y} width={barWidth} height={h} fill={v > 0 ? color : 'var(--ep-text-dim)'} />
  {/each}
</svg>

<style>
  .spark { display: inline-block; vertical-align: middle; }
</style>
