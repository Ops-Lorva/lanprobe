<script lang="ts">
  import { onMount } from 'svelte';
  import { _ } from 'svelte-i18n';
  import CodeDisplay from '$lib/components/CodeDisplay.svelte';
  import AdvertiseCard from '$lib/components/AdvertiseCard.svelte';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import { fleet } from '$lib/fleet';
  import { go } from '$lib/router';
  import { api, ApiError, type AdvertiseTest, type EnrollCode } from '$lib/api';

  const { onExpired } = $props<{ onExpired: () => void }>();

  const hubUrl = `${location.protocol}//${location.host}`;

  // Site éventuellement passé par l'écran vide d'un site : #/enroll?site=Durand
  const siteFromUrl = $derived.by(() => {
    const q = location.hash.split('?')[1] ?? '';
    const v = new URLSearchParams(q).get('site');
    return v ? decodeURIComponent(v) : null;
  });

  let siteId = $state('');
  let code = $state<EnrollCode | null>(null);
  let codeBusy = $state(false);
  let codeError = $state('');

  onMount(() => {
    if ($fleet.sites.length === 0) void fleet.load(onExpired, { quiet: true });
    void runTest();
  });

  // Présélection : le site demandé par l'URL, sinon le premier de la liste.
  $effect(() => {
    if (siteId || $fleet.sites.length === 0) return;
    const wanted = siteFromUrl
      ? $fleet.sites.find((s) => s.name.toLowerCase() === siteFromUrl.toLowerCase())
      : null;
    siteId = (wanted ?? $fleet.sites[0]).site_id;
  });

  async function generate() {
    if (!siteId) return;
    codeBusy = true;
    codeError = '';
    try {
      code = await api.createEnrollCode(siteId);
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      codeError = e instanceof ApiError ? e.message : String(e);
    } finally {
      codeBusy = false;
    }
  }

  // Vérification de l'URL d'Influx remise à la sonde : ici, l'erreur coûte
  // encore zéro. Découverte plus tard, elle prend la forme d'une sonde en
  // ligne qui n'écrit rien.
  let result = $state<AdvertiseTest | null>(null);
  let phase = $state<'idle' | 'testing' | 'done' | 'failed'>('idle');
  let advError = $state('');

  async function runTest() {
    phase = 'testing';
    advError = '';
    try {
      result = await api.testAdvertise();
      phase = 'done';
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      advError = e instanceof ApiError ? e.message : String(e);
      phase = 'failed';
    }
  }
</script>

<a class="back" href="#/">
  <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
    <polyline points="10,3 5,8 10,13" />
  </svg>
  {$_('enroll.back')}
</a>

<header class="head">
  <h1>{$_('enroll.title')}</h1>
  <p class="lead">{$_('enroll.lead')}</p>
</header>

<!--
  Le code est l'écran, pas un encart de l'écran. Deux valeurs, énormes,
  copiables d'un clic : c'est littéralement tout ce que la personne recopie sur
  l'autre machine. Le reste de la page explique, il ne s'interpose pas.
-->
<section class="code-card lp-card">
  <div class="ctop">
    <div>
      <h2 class="lp-title">{$_('code.title')}</h2>
      <p class="sub">{$_('code.lead')}</p>
    </div>
    <label class="site-pick lp-field">
      {$_('code.site_label')}
      <select bind:value={siteId} disabled={$fleet.sites.length === 0}>
        {#each $fleet.sites as s (s.site_id)}
          <option value={s.site_id}>{s.name}</option>
        {/each}
      </select>
    </label>
  </div>

  {#if $fleet.sites.length === 0}
    <StateBlock title={$_('fleet.empty_sites_title')} body={$_('code.no_site')}>
      <button class="lp-btn primary" onclick={() => go('/')}>{$_('enroll.back')}</button>
    </StateBlock>
  {:else if !code}
    <div class="cta">
      <button class="lp-btn primary" onclick={generate} disabled={codeBusy}>
        {codeBusy ? $_('code.generating') : $_('code.generate')}
      </button>
      <span class="muted">{$_('code.site_created')}</span>
    </div>
  {:else}
    <CodeDisplay
      code={code.code}
      expiresAt={code.expires_at}
      {hubUrl}
      dictate={$_('code.dictate_enroll')}
      onregenerate={generate}
      busy={codeBusy}
      regenerateLabel={$_('code.regenerate')}
    />
    <span class="muted">{$_('code.used_hint')}</span>
  {/if}

  {#if codeError}<p class="err" role="alert">{codeError}</p>{/if}
</section>

<section class="url lp-card">
  <span class="lp-label">{$_('enroll.advertise_title')}</span>
  <p class="advertise-body">{$_('enroll.advertise_body')}</p>
  <AdvertiseCard {result} {phase} error={advError} ontest={runTest} showFix />
</section>

<details class="manual">
  <summary>{$_('enroll.manual_title')}</summary>
  <p>{$_('enroll.manual_body')}</p>
</details>

<p class="note">{$_('enroll.note')}</p>

<style>
  .back {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    color: var(--ep-text-secondary);
    text-decoration: none;
    margin-bottom: 12px;
  }
  .back:hover {
    color: var(--ep-accent-bright);
  }

  .head {
    margin-bottom: 14px;
    max-width: 74ch;
  }
  h1 {
    font-size: 20px;
    font-weight: 800;
    letter-spacing: -0.02em;
    margin: 0;
  }
  .lead {
    font-size: 12.5px;
    line-height: 1.65;
    color: var(--ep-text-secondary);
    margin: 6px 0 0;
  }

  .code-card,
  .url {
    max-width: 760px;
    padding: 16px;
    margin-bottom: 12px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .ctop {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
  }
  .sub {
    font-size: 12px;
    line-height: 1.6;
    color: var(--ep-text-secondary);
    margin: 4px 0 0;
    max-width: 62ch;
  }
  .site-pick {
    flex-shrink: 0;
  }
  .site-pick select {
    min-width: 160px;
  }

  .cta {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  .muted {
    font-size: 11.5px;
    color: var(--ep-text-muted);
    max-width: 60ch;
    line-height: 1.5;
  }
  .advertise-body {
    font-size: 12px;
    line-height: 1.6;
    color: var(--ep-text-secondary);
    margin: -4px 0 0;
    max-width: 78ch;
  }

  .manual {
    max-width: 760px;
    margin-bottom: 14px;
    font-size: 12px;
  }
  .manual summary {
    cursor: pointer;
    color: var(--ep-text-secondary);
  }
  .manual p {
    font-size: 12px;
    line-height: 1.65;
    color: var(--ep-text-secondary);
    margin: 8px 0 0;
    max-width: 78ch;
  }

  .note {
    font-size: 11.5px;
    color: var(--ep-text-muted);
    line-height: 1.6;
    max-width: 74ch;
    border-left: 2px solid var(--ep-border-strong);
    padding-left: 10px;
  }
  .err {
    font-size: 12px;
    color: var(--ep-danger);
    margin: 0;
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
  }
</style>
