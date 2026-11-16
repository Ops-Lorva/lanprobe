<script lang="ts">
  const { id, icon, label }: { id: string; icon: string; label: string } = $props();
</script>

<div class="anchor" {id}>
  <span class="icon">{icon}</span>
  <h2 class="label">{label}</h2>
</div>

<style>
  .anchor {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 20px 24px 4px;
    border-top: 1px solid var(--ep-glass-border);
    background: var(--ep-bg-primary);
    position: sticky;
    top: 0;
    z-index: 2;
  }
  .anchor:first-of-type { border-top: none; }
  .icon { font-size: 18px; }
  .label { font-size: 12px; font-weight: 700; text-transform: uppercase; letter-spacing: 1px; color: var(--ep-text-muted); margin: 0; }
</style>
