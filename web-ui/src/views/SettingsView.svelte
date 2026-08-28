<script lang="ts">
  import { onMount } from 'svelte';
  import { _, locale } from 'svelte-i18n';
  import {
    api,
    ApiError,
    SETTINGS_DEFAULTS,
    type AdvertiseTest,
    type HubSettings,
    type InfluxInfo,
    type ReadToken,
  } from '$lib/api';
  import AdvertiseCard from '$lib/components/AdvertiseCard.svelte';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import CopyLine from '$lib/components/CopyLine.svelte';
  import { dateOnly, humanBytes } from '$lib/time';

  const { onExpired } = $props<{ onExpired: () => void }>();
  const lang = $derived($locale ?? 'en');

  let loading = $state(true);
  let loadError = $state('');
  let saved = $state<HubSettings>({ ...SETTINGS_DEFAULTS });

  // Brouillon : les champs numériques sont tenus en texte pour distinguer
  // « vidé » de « zéro » pendant la saisie.
  let influxUrl = $state('');
  let org = $state('');
  let bucket = $state('');
  let advertise = $state('');
  let retention = $state('0');
  let heartbeat = $state('60');

  let fieldError = $state('');
  let busy = $state(false);
  let notice = $state('');

  function fill(s: HubSettings) {
    saved = s;
    influxUrl = s.influx_url;
    org = s.influx_org;
    bucket = s.influx_bucket;
    advertise = s.influx_advertise_url ?? '';
    retention = String(s.retention_days);
    heartbeat = String(s.heartbeat_interval_secs);
  }

  async function load() {
    loading = true;
    loadError = '';
    try {
      fill(await api.settings());
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      loadError = e instanceof ApiError ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void load();
    void runTest();
    void loadInflux();
    void loadTokens();
  });

  // ── Influx en lecture seule (contrat § 8) ────────────────────────────────
  let influx = $state<InfluxInfo | null>(null);
  let influxError = $state('');

  async function loadInflux() {
    try {
      influx = await api.influx();
      influxError = '';
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      // Un état d'Influx indisponible ne doit pas emporter tout l'écran :
      // les autres réglages restent modifiables.
      influx = null;
      influxError = e instanceof ApiError ? e.message : String(e);
    }
  }

  // ── Jetons de lecture pour Grafana ───────────────────────────────────────
  let tokens = $state<ReadToken[]>([]);
  let tokensError = $state('');
  let tokenDesc = $state('');
  let tokenBusy = $state(false);
  /** Valeur en clair : affichée une seule fois, jamais relue depuis le hub. */
  let freshToken = $state<string | null>(null);

  async function loadTokens() {
    try {
      tokens = (await api.readTokens()) ?? [];
      tokensError = '';
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      tokens = [];
      tokensError = e instanceof ApiError ? e.message : String(e);
    }
  }

  async function createToken() {
    tokenBusy = true;
    tokensError = '';
    try {
      const created = await api.createReadToken(tokenDesc.trim());
      freshToken = created?.token ?? null;
      tokenDesc = '';
      await loadTokens();
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      tokensError = e instanceof ApiError ? e.message : String(e);
    } finally {
      tokenBusy = false;
    }
  }

  let revokeToken = $state<ReadToken | null>(null);
  let revokeBusy = $state(false);

  async function confirmRevokeToken() {
    if (!revokeToken) return;
    revokeBusy = true;
    try {
      await api.revokeReadToken(revokeToken.auth_id);
      revokeToken = null;
      await loadTokens();
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      tokensError = e instanceof ApiError ? e.message : String(e);
      revokeToken = null;
    } finally {
      revokeBusy = false;
    }
  }

  // ── Test de l'URL annoncée ───────────────────────────────────────────────
  let testResult = $state<AdvertiseTest | null>(null);
  let testPhase = $state<'idle' | 'testing' | 'done' | 'failed'>('idle');
  let testError = $state('');

  async function runTest() {
    testPhase = 'testing';
    testError = '';
    try {
      // `reachable: false` revient en 200 : c'est un verdict, pas une panne.
      testResult = await api.testAdvertise();
      testPhase = 'done';
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      testError = e instanceof ApiError ? e.message : String(e);
      testPhase = 'failed';
    }
  }

  // ── Enregistrement ───────────────────────────────────────────────────────
  const retentionNum = $derived(Number.parseInt(retention, 10));
  const heartbeatNum = $derived(Number.parseInt(heartbeat, 10));

  function retentionLabel(days: number) {
    return days === 0
      ? $_('settings.retention_unlimited_label')
      : $_('settings.retention_days_label', { values: { n: days } });
  }

  /**
   * Une réduction de rétention est le seul changement de cet écran qui détruit
   * des données. Passer de « illimité » à n'importe quelle valeur en est une :
   * 0 signifie « pour toujours », pas « zéro jour ».
   */
  const isShrink = $derived(
    Number.isInteger(retentionNum) &&
      retentionNum >= 0 &&
      ((saved.retention_days === 0 && retentionNum > 0) ||
        (saved.retention_days > 0 && retentionNum > 0 && retentionNum < saved.retention_days)),
  );

  const cutoffDate = $derived(
    Number.isInteger(retentionNum) && retentionNum > 0
      ? new Intl.DateTimeFormat(lang, { dateStyle: 'long' }).format(
          new Date(Date.now() - retentionNum * 86_400_000),
        )
      : '',
  );

  const patch = $derived.by(() => {
    const p: Partial<HubSettings> = {};
    if (influxUrl.trim() !== saved.influx_url) p.influx_url = influxUrl.trim();
    if (org.trim() !== saved.influx_org) p.influx_org = org.trim();
    if (bucket.trim() !== saved.influx_bucket) p.influx_bucket = bucket.trim();
    const adv = advertise.trim() === '' ? null : advertise.trim();
    if (adv !== saved.influx_advertise_url) p.influx_advertise_url = adv;
    if (retentionNum !== saved.retention_days) p.retention_days = retentionNum;
    if (heartbeatNum !== saved.heartbeat_interval_secs) p.heartbeat_interval_secs = heartbeatNum;
    return p;
  });

  const dirty = $derived(Object.keys(patch).length > 0);

  // ── Onglets ──────────────────────────────────────────────────────────────
  // Découpage par motif de visite, pas par type de champ. On vient ici pour
  // l'une de ces quatre raisons, jamais pour deux à la fois :
  //   « Sondes »       — ce que le hub raconte aux sondes (URL annoncée, cadence) ;
  //   « InfluxDB »     — comment le hub joint la base, et comment elle est servie ;
  //   « Vos données »  — ce qu'il y a dedans et qui a le droit de le lire ;
  //   « Conservation » — le seul réglage qui détruit des mesures, donc isolé.
  // Un seul bouton d'enregistrement pour l'ensemble : le contrat n'expose qu'un
  // PATCH, le découper en quatre formulaires mentirait sur ce qui part au hub.
  // La pastille sur un onglet dit où sont les modifications en attente — sans
  // elle, on enregistrerait une valeur saisie sur un onglet qu'on ne voit plus.
  type Tab = 'probes' | 'influx' | 'data' | 'retention';
  const TABS: { id: Tab; key: string }[] = [
    { id: 'probes', key: 'settings.tab_probes' },
    { id: 'influx', key: 'settings.tab_influx' },
    { id: 'data', key: 'settings.influx_state_title' },
    { id: 'retention', key: 'settings.tab_retention' },
  ];
  let tab = $state<Tab>('probes');

  const dirtyTabs = $derived<Record<Tab, boolean>>({
    probes: 'influx_advertise_url' in patch || 'heartbeat_interval_secs' in patch,
    influx: 'influx_url' in patch || 'influx_org' in patch || 'influx_bucket' in patch,
    // Onglet en lecture seule : les jetons s'y créent et s'y révoquent tout de
    // suite, rien n'y attend le bouton « Enregistrer ».
    data: false,
    retention: 'retention_days' in patch,
  });

  function onTabKey(e: KeyboardEvent) {
    const i = TABS.findIndex((t) => t.id === tab);
    if (e.key === 'ArrowRight') tab = TABS[(i + 1) % TABS.length].id;
    else if (e.key === 'ArrowLeft') tab = TABS[(i - 1 + TABS.length) % TABS.length].id;
    else return;
    e.preventDefault();
    (e.currentTarget as HTMLElement)
      .querySelector<HTMLElement>(`[data-tab="${tab}"]`)
      ?.focus();
  }

  /** Le message ET l'onglet où se trouve le champ fautif : une erreur sur un
      onglet fermé serait invisible, donc incompréhensible. */
  function validate(): { msg: string; tab: Tab } | null {
    if (!influxUrl.trim() || !org.trim() || !bucket.trim())
      return { msg: $_('settings.required'), tab: 'influx' };
    try {
      new URL(influxUrl.trim());
    } catch {
      return { msg: $_('settings.invalid_url'), tab: 'influx' };
    }
    if (advertise.trim()) {
      try {
        new URL(advertise.trim());
      } catch {
        return { msg: $_('settings.invalid_url'), tab: 'probes' };
      }
    }
    if (!Number.isInteger(retentionNum) || retentionNum < 0)
      return { msg: $_('settings.invalid_retention'), tab: 'retention' };
    if (!Number.isInteger(heartbeatNum) || heartbeatNum < 10)
      return { msg: $_('settings.invalid_heartbeat'), tab: 'probes' };
    return null;
  }

  let shrinkOpen = $state(false);
  let shrinkAck = $state(false);

  function onSave() {
    notice = '';
    const bad = validate();
    fieldError = bad?.msg ?? '';
    if (bad) {
      tab = bad.tab;
      return;
    }
    if (!dirty) {
      notice = $_('settings.no_change');
      return;
    }
    if (isShrink) {
      // Confirmation explicite exigée : le clic sur « Enregistrer » peut être
      // un réflexe, cocher une case qui nomme la date de coupure ne l'est pas.
      shrinkAck = false;
      shrinkOpen = true;
      return;
    }
    void commit(false);
  }

  async function commit(confirmed: boolean) {
    busy = true;
    fieldError = '';
    try {
      await api.saveSettings(
        confirmed ? { ...patch, confirm_data_loss: true } : patch,
      );
      shrinkOpen = false;
      notice = $_('settings.saved_all');
      await load();
      await runTest();
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      fieldError = e instanceof ApiError ? e.message : String(e);
      shrinkOpen = false;
    } finally {
      busy = false;
    }
  }
</script>

<header class="head">
  <h1>{$_('settings.title')}</h1>
  <p class="lead">{$_('settings.lead')}</p>
</header>

{#if loading}
  <StateBlock tone="loading" title={$_('settings.loading')} />
{:else if loadError}
  <StateBlock tone="error" title={$_('settings.error_title')} body={loadError}>
    <button class="lp-btn primary" onclick={load}>{$_('common.retry')}</button>
  </StateBlock>
{:else}
  <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
  <div class="tabs" role="tablist" aria-label={$_('settings.tabs_aria')} onkeydown={onTabKey}>
    {#each TABS as t (t.id)}
      <button
        class="tab"
        class:on={tab === t.id}
        role="tab"
        data-tab={t.id}
        id="tab-{t.id}"
        aria-selected={tab === t.id}
        aria-controls="panel-{t.id}"
        tabindex={tab === t.id ? 0 : -1}
        onclick={() => (tab = t.id)}
      >
        {$_(t.key)}
        {#if dirtyTabs[t.id]}
          <span class="dot" title={$_('settings.tab_dirty')} aria-label={$_('settings.tab_dirty')}
          ></span>
        {/if}
      </button>
    {/each}
  </div>

  {#if tab === 'probes'}
  <div class="cards" role="tabpanel" id="panel-probes" aria-labelledby="tab-probes" tabindex="-1">
    <!-- L'URL annoncée vient en premier : c'est celle qui casse le parc quand
         elle est fausse, et la seule qu'on puisse vérifier d'un bouton. -->
    <section class="card lp-card">
      <h2 class="lp-title">{$_('settings.influx_title')}</h2>
      <p class="sub">{$_('settings.influx_lead')}</p>

      <label class="lp-field">
        {$_('settings.field_label')}
        <input
          class="lp-input"
          bind:value={advertise}
          placeholder={$_('settings.field_placeholder')}
          spellcheck="false"
          autocomplete="off"
        />
        <span class="hint">{$_('settings.field_hint')}</span>
      </label>

      <AdvertiseCard
        result={testResult}
        phase={testPhase}
        error={testError}
        ontest={runTest}
      />

      <details class="why">
        <summary>{$_('settings.why_title')}</summary>
        <p>{$_('settings.why_body')}</p>
        <p class="ord-title">{$_('settings.order_title')}</p>
        <ol>
          <li>{$_('settings.order_1')}</li>
          <li>{$_('settings.order_2')}</li>
          <li>{$_('settings.order_3')}</li>
        </ol>
      </details>
    </section>

    <section class="card lp-card">
      <h2 class="lp-title">{$_('settings.section_probes')}</h2>
      <label class="lp-field">
        {$_('settings.heartbeat_label')}
        <span class="unit-row">
          <input class="lp-input num" type="number" min="10" step="1" bind:value={heartbeat} />
          <span class="unit">{$_('settings.heartbeat_unit')}</span>
        </span>
        <span class="hint">{$_('settings.heartbeat_hint')}</span>
      </label>
    </section>
  </div>
  {:else if tab === 'influx'}
  <div class="cards" role="tabpanel" id="panel-influx" aria-labelledby="tab-influx" tabindex="-1">
    <section class="card lp-card">
      <h2 class="lp-title">{$_('settings.section_influx')}</h2>
      <label class="lp-field">
        {$_('settings.influx_url_label')}
        <input class="lp-input" bind:value={influxUrl} spellcheck="false" autocomplete="off" />
        <span class="hint">{$_('settings.influx_url_hint')}</span>
      </label>
      <div class="pair">
        <label class="lp-field">
          {$_('settings.org_label')}
          <input class="lp-input" bind:value={org} spellcheck="false" autocomplete="off" />
        </label>
        <label class="lp-field">
          {$_('settings.bucket_label')}
          <input class="lp-input" bind:value={bucket} spellcheck="false" autocomplete="off" />
        </label>
      </div>
      <p class="tls">
        <strong>{$_('settings.tls_title')}</strong>
        {$_('settings.tls_body')}
      </p>
    </section>
  </div>
  {:else if tab === 'data'}
  <div class="cards" role="tabpanel" id="panel-data" aria-labelledby="tab-data" tabindex="-1">
    <!--
      Influx en lecture seule. L'objectif n'est pas d'administrer la base, mais
      de savoir où sont ses mesures et si elles vont bien — sans jamais ouvrir
      la console d'InfluxDB.
    -->
    <section class="card lp-card">
      <h2 class="lp-title">{$_('settings.influx_state_title')}</h2>
      <p class="sub">{$_('settings.influx_state_lead')}</p>

      {#if influx}
        <dl class="stats">
          <div>
            <dt>{$_('settings.state_health')}</dt>
            <dd class:ok={influx.healthy === true} class:bad={influx.healthy === false}>
              {influx.healthy == null
                ? '—'
                : influx.healthy
                  ? $_('settings.state_healthy')
                  : $_('settings.state_unhealthy')}
            </dd>
          </div>
          <div>
            <dt>{$_('settings.state_version')}</dt>
            <dd class="lp-mono">{influx.version ?? '—'}</dd>
          </div>
          <div>
            <dt>{$_('settings.state_size')}</dt>
            <dd class="lp-mono">{humanBytes(influx.size_bytes, lang)}</dd>
          </div>
          <div>
            <dt>{$_('settings.state_series')}</dt>
            <dd class="lp-mono">
              {influx.series_count != null
                ? new Intl.NumberFormat(lang).format(influx.series_count)
                : '—'}
            </dd>
          </div>
          <div>
            <dt>{$_('settings.org_label')}</dt>
            <dd class="lp-mono">{influx.org || '—'}</dd>
          </div>
          <div>
            <dt>{$_('settings.bucket_label')}</dt>
            <dd class="lp-mono">{influx.bucket || '—'}</dd>
          </div>
        </dl>
      {:else}
        <p class="muted">{$_('settings.state_unavailable')}{influxError ? ` (${influxError})` : ''}</p>
      {/if}

      <div class="tokens">
        <h3>{$_('settings.tokens_title')}</h3>
        <p class="sub">{$_('settings.tokens_lead')}</p>
        <p class="ro">
          {$_('settings.tokens_readonly', {
            values: { bucket: influx?.bucket || bucket || 'lanprobe' },
          })}
        </p>

        <div class="tok-form">
          <label class="lp-field grow">
            {$_('settings.tokens_desc_label')}
            <input
              class="lp-input"
              bind:value={tokenDesc}
              placeholder={$_('settings.tokens_desc_placeholder')}
            />
          </label>
          <button class="lp-btn accent" onclick={createToken} disabled={tokenBusy}>
            {tokenBusy ? $_('settings.tokens_creating') : $_('settings.tokens_create')}
          </button>
        </div>

        {#if tokensError}<p class="err" role="alert">{tokensError}</p>{/if}

        {#if tokens.length === 0}
          <p class="muted">{$_('settings.tokens_none')}</p>
        {:else}
          <!-- La liste ne montre JAMAIS la valeur : identifiant et description
               suffisent pour savoir lequel révoquer. -->
          <table class="tok-table">
            <thead>
              <tr>
                <th>{$_('settings.tokens_col_desc')}</th>
                <th>{$_('settings.tokens_col_id')}</th>
                <th>{$_('settings.tokens_col_created')}</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {#each tokens as t (t.auth_id)}
                <tr>
                  <td>{t.description || '—'}</td>
                  <td class="lp-mono id">{t.id}</td>
                  <td class="lp-mono">{dateOnly(t.created_at, lang)}</td>
                  <td class="right">
                    <button class="lp-btn danger sm" onclick={() => (revokeToken = t)}>
                      {$_('settings.tokens_revoke')}
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </div>
    </section>
  </div>
  {:else}
  <div
    class="cards"
    role="tabpanel"
    id="panel-retention"
    aria-labelledby="tab-retention"
    tabindex="-1"
  >
    <section class="card lp-card" class:risky={isShrink}>
      <h2 class="lp-title">{$_('settings.section_retention')}</h2>
      <label class="lp-field">
        {$_('settings.retention_label')}
        <span class="unit-row">
          <input class="lp-input num" type="number" min="0" step="1" bind:value={retention} />
          <span class="unit">{$_('settings.retention_unit')}</span>
        </span>
        <span class="hint">{$_('settings.retention_hint')}</span>
        <span class="cur">
          {$_('settings.retention_current', {
            values: { label: retentionLabel(saved.retention_days) },
          })}
        </span>
      </label>

      {#if isShrink}
        <!-- L'avertissement apparaît pendant la saisie, pas seulement au clic :
             on veut que la personne change d'avis avant, pas qu'elle découvre
             la conséquence dans une boîte de dialogue. -->
        <div class="warn" role="status">
          <strong>{$_('settings.shrink_title')}</strong>
          <p>
            {$_('settings.shrink_from_to', {
              values: {
                from: retentionLabel(saved.retention_days),
                to: retentionLabel(retentionNum),
              },
            })}
            {$_('settings.shrink_cutoff', { values: { date: cutoffDate } })}
          </p>
        </div>
      {/if}
    </section>
  </div>
  {/if}

  <div class="save-row">
    {#if fieldError}<p class="err" role="alert">{fieldError}</p>{/if}
    {#if notice}<p class="ok" role="status">{notice}</p>{/if}
    <button class="lp-btn primary" onclick={onSave} disabled={busy || !dirty}>
      {busy ? $_('settings.saving_all') : $_('settings.save_all')}
    </button>
  </div>

  <!-- Jeton affiché une seule fois : le dire avant de le montrer, pas après. -->
  <Modal
    open={freshToken !== null}
    wide
    title={$_('settings.tokens_once_title')}
    onclose={() => (freshToken = null)}
  >
    <p>{$_('settings.tokens_once_body')}</p>
    {#if freshToken}<CopyLine value={freshToken} shell={false} />{/if}
    <p class="muted">
      {$_('settings.tokens_readonly', {
        values: { bucket: influx?.bucket || bucket || 'lanprobe' },
      })}
    </p>
    {#snippet footer()}
      <button class="lp-btn primary" onclick={() => (freshToken = null)}>
        {$_('settings.tokens_once_ack')}
      </button>
    {/snippet}
  </Modal>

  <Modal
    open={revokeToken !== null}
    title={$_('settings.tokens_revoke_title')}
    onclose={() => (revokeToken = null)}
  >
    <p>{$_('settings.tokens_revoke_body')}</p>
    <p class="lp-mono id">{revokeToken?.description || revokeToken?.auth_id}</p>
    {#snippet footer()}
      <button class="lp-btn" onclick={() => (revokeToken = null)}>{$_('common.cancel')}</button>
      <button class="lp-btn danger" onclick={confirmRevokeToken} disabled={revokeBusy}>
        {revokeBusy ? $_('settings.tokens_revoking') : $_('settings.tokens_revoke_confirm')}
      </button>
    {/snippet}
  </Modal>

  <Modal open={shrinkOpen} title={$_('settings.shrink_title')} onclose={() => (shrinkOpen = false)}>
    <p>
      {$_('settings.shrink_from_to', {
        values: {
          from: retentionLabel(saved.retention_days),
          to: retentionLabel(retentionNum),
        },
      })}
    </p>
    <p class="cutoff">{$_('settings.shrink_cutoff', { values: { date: cutoffDate } })}</p>
    <p class="muted">{$_('settings.shrink_irreversible')}</p>

    <label class="ack">
      <input type="checkbox" bind:checked={shrinkAck} />
      <span>{$_('settings.shrink_ack', { values: { date: cutoffDate } })}</span>
    </label>

    {#snippet footer()}
      <button class="lp-btn" onclick={() => (shrinkOpen = false)}>
        {$_('settings.shrink_keep', { values: { from: retentionLabel(saved.retention_days) } })}
      </button>
      <button class="lp-btn danger" onclick={() => commit(true)} disabled={!shrinkAck || busy}>
        {busy ? $_('settings.saving_all') : $_('settings.shrink_confirm')}
      </button>
    {/snippet}
  </Modal>
{/if}

<style>
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

  /* Onglets soulignés plutôt qu'en pastilles : le trait actif prolonge la ligne
     qui sépare la barre du contenu, donc l'onglet et son panneau se lisent
     comme une seule pièce. */
  .tabs {
    display: flex;
    gap: 2px;
    flex-wrap: wrap;
    border-bottom: 1px solid var(--ep-border);
    margin-bottom: 14px;
    max-width: 760px;
  }
  .tab {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 8px 12px;
    margin-bottom: -1px;
    border: none;
    border-bottom: 2px solid transparent;
    background: transparent;
    color: var(--ep-text-secondary);
    font-family: var(--ep-font-sans);
    font-size: 12.5px;
    font-weight: 500;
    cursor: pointer;
    border-radius: var(--ep-radius-sm) var(--ep-radius-sm) 0 0;
  }
  .tab:hover:not(.on) {
    color: var(--ep-text-primary);
    background: var(--ep-glass-bg-md);
  }
  .tab.on {
    color: var(--ep-accent-bright);
    border-bottom-color: var(--ep-accent);
    font-weight: 600;
  }
  /* Une modification en attente sur un onglet fermé serait enregistrée sans
     avoir été revue : la pastille dit où elle est. */
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: var(--ep-warning);
    flex-shrink: 0;
  }

  .cards {
    display: flex;
    flex-direction: column;
    gap: 12px;
    max-width: 760px;
  }
  .cards:focus {
    outline: none;
  }
  .card {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .card.risky {
    border-color: color-mix(in srgb, var(--ep-danger) 45%, var(--ep-border));
  }
  .sub {
    font-size: 12px;
    color: var(--ep-text-secondary);
    line-height: 1.6;
    margin: -6px 0 0;
    max-width: 74ch;
  }
  .hint {
    font-size: 11px;
    color: var(--ep-text-muted);
    line-height: 1.55;
    max-width: 74ch;
  }
  .cur {
    font-size: 11px;
    color: var(--ep-text-secondary);
  }
  .pair {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 12px;
  }
  .unit-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .num {
    width: 110px;
    font-variant-numeric: tabular-nums;
  }
  .unit {
    font-size: 12px;
    color: var(--ep-text-muted);
  }

  .tls {
    font-size: 11.5px;
    color: var(--ep-text-muted);
    line-height: 1.6;
    margin: 0;
    border-left: 2px solid var(--ep-border-strong);
    padding-left: 10px;
    max-width: 78ch;
  }
  .tls strong {
    color: var(--ep-text-secondary);
    display: block;
  }

  .why {
    font-size: 12px;
    color: var(--ep-text-secondary);
  }
  .why summary {
    cursor: pointer;
    color: var(--ep-text-secondary);
    font-size: 12px;
  }
  .why p,
  .why ol {
    margin: 8px 0 0;
    line-height: 1.6;
    max-width: 78ch;
  }
  .why ol {
    padding-left: 20px;
  }
  .ord-title {
    font-weight: 600;
    color: var(--ep-text-primary);
  }

  .warn {
    border: 1px solid color-mix(in srgb, var(--ep-danger) 45%, var(--ep-border));
    border-left-width: 3px;
    border-radius: var(--ep-radius-md);
    background: color-mix(in srgb, var(--ep-danger) 7%, transparent);
    padding: 11px 13px;
  }
  .warn strong {
    font-size: 12.5px;
    color: var(--ep-danger);
    display: block;
    margin-bottom: 4px;
  }
  .warn p {
    font-size: 11.5px;
    color: var(--ep-text-secondary);
    line-height: 1.6;
    margin: 0;
    max-width: 78ch;
  }

  .stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
    gap: 6px;
    margin: 0;
  }
  .stats div {
    background: var(--ep-bg-tertiary);
    border: 1px solid var(--ep-border);
    border-radius: var(--ep-radius-md);
    padding: 9px 11px;
    min-width: 0;
  }
  .stats dt {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--ep-text-dim);
    margin-bottom: 3px;
  }
  .stats dd {
    margin: 0;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--ep-text-primary);
    overflow-wrap: anywhere;
  }
  .stats dd.ok {
    color: var(--ep-success);
  }
  .stats dd.bad {
    color: var(--ep-danger);
  }

  .tokens {
    border-top: 1px solid var(--ep-border);
    padding-top: 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .tokens h3 {
    font-size: 13px;
    font-weight: 700;
    margin: 0;
    color: var(--ep-text-primary);
  }
  .ro {
    font-size: 11.5px;
    color: var(--ep-success);
    margin: 0;
  }
  .tok-form {
    display: flex;
    align-items: flex-end;
    gap: 10px;
    flex-wrap: wrap;
  }
  .grow {
    flex: 1 1 220px;
    min-width: 0;
  }
  .tok-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 11.5px;
  }
  .tok-table th {
    text-align: left;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--ep-text-dim);
    padding: 5px 8px;
    border-bottom: 1px solid var(--ep-border);
    font-weight: 500;
  }
  .tok-table td {
    padding: 7px 8px;
    border-bottom: 1px solid var(--ep-border);
    color: var(--ep-text-secondary);
  }
  .tok-table tr:last-child td {
    border-bottom: none;
  }
  .tok-table .right {
    text-align: right;
  }
  .id {
    font-size: 10.5px;
    color: var(--ep-text-muted);
    overflow-wrap: anywhere;
  }

  .save-row {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    margin-top: 14px;
    max-width: 760px;
  }
  .save-row button {
    margin-left: auto;
  }
  .err {
    font-size: 12px;
    color: var(--ep-danger);
    margin: 0;
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
  }
  .ok {
    font-size: 12px;
    color: var(--ep-success);
    margin: 0;
  }

  .cutoff {
    color: var(--ep-text-primary);
    border-left: 2px solid var(--ep-danger);
    padding-left: 10px;
  }
  .muted {
    color: var(--ep-text-muted);
    font-size: 12px;
  }
  .ack {
    display: flex;
    gap: 9px;
    align-items: flex-start;
    font-size: 12px;
    color: var(--ep-text-primary);
    line-height: 1.5;
    cursor: pointer;
    border-top: 1px solid var(--ep-border);
    padding-top: 12px;
  }
  .ack input {
    margin-top: 2px;
    accent-color: var(--ep-danger);
    width: 15px;
    height: 15px;
    flex-shrink: 0;
  }
</style>
