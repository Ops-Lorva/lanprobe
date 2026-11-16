<script lang="ts">
  import { onMount } from 'svelte';
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

  // Intervalles proposés (minutes) ; 0 = désactivé (valeur par défaut).
  const OPTIONS = [0, 5, 10, 15, 30, 60];

  let value = $state<number>(0);

  // Idempotent : le contrôle vit aussi au tableau de bord et dans les
  // réglages, où personne d'autre ne charge le planificateur.
  onMount(() => { scheduler.init(); });

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

<label class="sched" title={$_('scheduler.auto_run')}>
  <span class="sched-label">{label || $_('scheduler.every')}</span>
  <select class="select" bind:value onchange={save}>
    {#each OPTIONS as opt}
      <option value={opt}>{opt === 0 ? $_('scheduler.off') : `${opt} min`}</option>
    {/each}
  </select>
</label>

<style>
  .sched { display: inline-flex; align-items: center; gap: var(--space-xs); font-size: var(--text-sm); }
  .sched-label { color: var(--color-muted); white-space: nowrap; }
  .sched .select { width: auto; min-height: 30px; }
</style>
