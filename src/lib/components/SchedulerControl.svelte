<script lang="ts">
  import { _ } from 'svelte-i18n';
  import { get } from 'svelte/store';
  import { scheduler, type SchedulerConfig } from '../stores/scheduler';

  // Contrôle compact « exécution automatique toutes les N min » pour UNE sonde.
  // Placé directement dans l'interface de la sonde (Discovery / PortScan / SpeedTest)
  // plutôt que dans les Settings. 0 = désactivé.
  let { field, label = '' }: {
    field: 'speedtest_interval_min' | 'discovery_interval_min' | 'portscan_interval_min';
    label?: string;
  } = $props();

  let value = $state<number>(0);

  // Reflète la valeur du store (après chargement async de la config).
  $effect(() => {
    const v = ($scheduler as SchedulerConfig)[field];
    if (typeof v === 'number') value = v;
  });

  async function save() {
    const cur = get(scheduler);
    const next = Math.max(0, Math.floor(Number(value) || 0));
    if (next === cur[field]) return;
    await scheduler.save({ ...cur, [field]: next });
  }
</script>

<div class="sched" title={$_('scheduler.auto_run')}>
  <span class="sched-icon">⏱</span>
  <span class="sched-label">{label || $_('scheduler.auto_run')}</span>
  <input
    type="number"
    min="0"
    bind:value
    onblur={save}
    onkeydown={(e) => { if (e.key === 'Enter') (e.currentTarget as HTMLInputElement).blur(); }}
  />
  <span class="sched-unit">{$_('scheduler.unit')}</span>
</div>

<style>
  .sched {
    display: inline-flex; align-items: center; gap: 6px;
    padding: 5px 10px; border-radius: 8px;
    background: var(--ep-bg-tertiary); border: 1px solid var(--ep-border);
    font-size: 12px;
  }
  .sched-icon { color: var(--ep-text-muted); }
  .sched-label { color: var(--ep-text-secondary); font-weight: 600; white-space: nowrap; }
  .sched input {
    width: 56px; background: var(--ep-bg-secondary); border: 1px solid var(--ep-border);
    border-radius: 6px; padding: 3px 6px; color: var(--ep-text-primary);
    font-size: 12px; font-weight: 600; text-align: center;
  }
  .sched input:focus { outline: none; border-color: var(--ep-accent); }
  .sched-unit { color: var(--ep-text-muted); white-space: nowrap; }
</style>
