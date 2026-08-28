<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
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
  import StateBlock from '$lib/components/StateBlock.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import CopyLine from '$lib/components/CopyLine.svelte';
  import LangTheme from '$lib/components/LangTheme.svelte';
  import AccountsView from './AccountsView.svelte';
  import NotificationsView from './NotificationsView.svelte';
  import { go, route, type SettingsTab } from '$lib/router';
  import { canOperate, isAdmin } from '$lib/session';
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
  let hubPublicUrl = $state('');
  // La version vient de /api/status, la seule route publique du hub.
  let hubVersion = $state('—');
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
    hubPublicUrl = s.hub_public_url ?? '';
    api.status().then((st) => (hubVersion = st.version || '—')).catch(() => {});
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
    void loadInflux();
    // Ces deux-là ont un rôle minimum (contrat § 11) : le test d'URL annoncée
    // demande `operator`, les jetons de lecture `admin`. Les appeler pour tout
    // le monde déposait une ligne `access.denied` au journal à chaque visite
    // d'un observateur — le bruit exact qui masque les refus qui comptent.
    if (get(canOperate)) void runTest();
    if (get(isAdmin)) void loadTokens();
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
    const hubUrl = hubPublicUrl.trim() === '' ? null : hubPublicUrl.trim();
    if (hubUrl !== saved.hub_public_url) p.hub_public_url = hubUrl;
    if (retentionNum !== saved.retention_days) p.retention_days = retentionNum;
    if (heartbeatNum !== saved.heartbeat_interval_secs) p.heartbeat_interval_secs = heartbeatNum;
    return p;
  });

  const dirty = $derived(Object.keys(patch).length > 0);

  // ── Onglets ──────────────────────────────────────────────────────────────
  // Découpage par MOTIF DE VISITE, pas par type de champ. Un réglage se cherche
  // avec une question en tête, pas avec une catégorie : ranger par nature de
  // champ produit un classeur qu'il faut connaître pour s'en servir.
  //
  //   « Général »  — comment ce hub est réglé : l'adresse par laquelle les
  //                  sondes le joignent, la cadence qu'il leur demande, sa
  //                  version, plus la langue et le thème de CE navigateur.
  //   « Stockage » — où vont les mesures, ce qu'il y a dedans, qui peut les
  //                  lire de l'extérieur, et combien de temps on les garde.
  //
  // « InfluxDB » et « Vos données » ont fusionné : les deux répondaient à la
  // même question — « mes mesures ». L'un portait l'adresse de la base, l'autre
  // son contenu, et il fallait déjà savoir lequel des deux tient le bucket pour
  // le trouver. Le contrat § 8 pose que personne ne doit avoir à administrer
  // Influx ; le nom du produit ne fait donc pas un intitulé d'onglet.
  //
  // « Sondes » a rejoint « Général » : deux champs — l'adresse du hub et la
  // cadence — ne remplissaient pas un onglet, et l'adresse du hub est ce qu'on
  // vient corriger quand une sonde n'apparaît pas, c'est-à-dire au premier
  // endroit qu'on ouvre.
  //
  // Un seul bouton d'enregistrement pour l'ensemble : le contrat n'expose qu'un
  // PATCH, le découper en formulaires mentirait sur ce qui part au hub.
  // La pastille sur un onglet dit où sont les modifications en attente — sans
  // elle, on enregistrerait une valeur saisie sur un onglet qu'on ne voit plus.
  type Tab = SettingsTab;
  const ALL_TABS: { id: Tab; key: string; admin?: true }[] = [
    { id: 'general', key: 'settings.tab_general' },
    { id: 'alerts', key: 'settings.tab_alerts' },
    { id: 'storage', key: 'settings.tab_storage' },
    { id: 'accounts', key: 'settings.tab_accounts', admin: true },
  ];

  // Un onglet qu'un rôle ne peut pas utiliser ne s'affiche pas pour lui — les
  // comptes sont réservés à `admin` (contrat § 11). Ce n'est PAS la protection :
  // elle est côté serveur, route par route. C'est du confort de navigation.
  const TABS = $derived(ALL_TABS.filter((t) => !t.admin || $isAdmin));

  // L'onglet vient de l'URL, pas d'un état local : « ouvre #/settings/storage »
  // se dicte au téléphone, « le troisième onglet » non. Un onglet demandé mais
  // indisponible retombe sur le premier plutôt que d'afficher un panneau que la
  // rangée ne montre pas.
  const asked = $derived($route.name === 'settings' ? $route.tab : 'general');
  const tab = $derived<Tab>(TABS.some((t) => t.id === asked) ? asked : 'general');

  function openTab(id: Tab) {
    go(`#/settings/${id}`);
  }

  const dirtyTabs = $derived<Record<Tab, boolean>>({
    // Langue et thème s'appliquent au clic et ne partent jamais au hub : rien à
    // y enregistrer de leur fait.
    general: 'hub_public_url' in patch || 'heartbeat_interval_secs' in patch,
    storage:
      'influx_url' in patch ||
      'influx_org' in patch ||
      'influx_bucket' in patch ||
      'retention_days' in patch,
    // Chaque geste s'y applique seul, rien n'y attend « Enregistrer ».
    alerts: false,
    accounts: false,
  });

  /**
   * Les onglets qui portent des champs partant au hub par `PUT /api/settings`.
   * Ailleurs (langue et thème, jetons, comptes, canaux), chaque geste s'applique
   * seul : un bouton « Enregistrer » y laisserait croire qu'il reste à valider.
   */
  const savable = $derived(tab === 'general' || tab === 'storage');

  function onTabKey(e: KeyboardEvent) {
    const i = TABS.findIndex((t) => t.id === tab);
    let next: Tab;
    if (e.key === 'ArrowRight') next = TABS[(i + 1) % TABS.length].id;
    else if (e.key === 'ArrowLeft') next = TABS[(i - 1 + TABS.length) % TABS.length].id;
    else return;
    e.preventDefault();
    openTab(next);
    (e.currentTarget as HTMLElement).querySelector<HTMLElement>(`[data-tab="${next}"]`)?.focus();
  }

  /** Le message ET l'onglet où se trouve le champ fautif : une erreur sur un
      onglet fermé serait invisible, donc incompréhensible. */
  function validate(): { msg: string; tab: Tab } | null {
    if (!influxUrl.trim() || !org.trim() || !bucket.trim())
      return { msg: $_('settings.required'), tab: 'storage' };
    try {
      new URL(influxUrl.trim());
    } catch {
      return { msg: $_('settings.invalid_url'), tab: 'storage' };
    }
    if (hubPublicUrl.trim()) {
      try {
        new URL(hubPublicUrl.trim());
      } catch {
        return { msg: $_('settings.invalid_url'), tab: 'general' };
      }
    }
    if (!Number.isInteger(retentionNum) || retentionNum < 0)
      return { msg: $_('settings.invalid_retention'), tab: 'storage' };
    if (!Number.isInteger(heartbeatNum) || heartbeatNum < 10)
      return { msg: $_('settings.invalid_heartbeat'), tab: 'general' };
    return null;
  }

  let shrinkOpen = $state(false);
  let shrinkAck = $state(false);

  function onSave() {
    notice = '';
    const bad = validate();
    fieldError = bad?.msg ?? '';
    if (bad) {
      openTab(bad.tab);
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
</header>

{#if loading}
  <StateBlock tone="loading" title={$_('settings.loading')} />
{:else if loadError}
  <StateBlock tone="error" title={$_('settings.error_title')} body={loadError}>
    <button class="lp-btn primary" onclick={load}>{$_('common.retry')}</button>
  </StateBlock>
{:else}
  <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
  <div
    class="tabs"
    role="tablist"
    tabindex="-1"
    aria-label={$_('settings.tabs_aria')}
    onkeydown={onTabKey}
  >
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
        onclick={() => openTab(t.id)}
      >
        {$_(t.key)}
        {#if dirtyTabs[t.id]}
          <span class="dot" title={$_('settings.tab_dirty')} aria-label={$_('settings.tab_dirty')}
          ></span>
        {/if}
      </button>
    {/each}
  </div>

  <!-- `GET /api/settings` est ouverte à `viewer`, son écriture est réservée à
       `admin` (contrat § 11). On montre donc les valeurs — c'est souvent ce
       qu'on vient vérifier — mais on dit tout de suite qu'elles ne partiront
       pas, plutôt que de laisser saisir puis refuser. -->
  {#if !$isAdmin && (tab === 'general' || tab === 'storage')}
    <p class="readonly">{$_('settings.readonly_note')}</p>
  {/if}

  {#if tab === 'general'}
  <div class="cards" role="tabpanel" id="panel-general" aria-labelledby="tab-general" tabindex="-1">
    <!-- L'adresse du hub ouvre l'onglet parce que c'est ce qu'on vient
         corriger : une sonde qui n'apparaît jamais, c'est presque toujours
         elle. Une seule adresse à renseigner — les sondes ne reçoivent plus
         d'URL InfluxDB, elles écrivent par le hub, qui relaie. Deux URL
         voisines à distinguer, c'était une sonde enrôlée avec la mauvaise et
         une demi-heure à comprendre pourquoi. -->
    <section class="card lp-card">
      <h2 class="lp-title">{$_('settings.hub_url_title')}</h2>
      <label class="lp-field">
        {$_('settings.hub_url_label')}
        <input
          class="lp-input"
          bind:value={hubPublicUrl}
          placeholder="https://lanprobe.exemple.fr"
          spellcheck="false"
          autocomplete="off"
          disabled={!$isAdmin}
        />
        <span class="hint">{$_('settings.hub_url_hint')}</span>
      </label>
    </section>

    <section class="card lp-card">
      <h2 class="lp-title">{$_('settings.section_probes')}</h2>
      <label class="lp-field">
        {$_('settings.heartbeat_label')}
        <span class="unit-row">
          <input
            class="lp-input num"
            type="number"
            min="10"
            step="1"
            bind:value={heartbeat}
            disabled={!$isAdmin}
          />
          <span class="unit">{$_('settings.heartbeat_unit')}</span>
        </span>
      </label>
    </section>

    <!--
      Langue et thème sont les seuls réglages de cet écran qui ne partent pas au
      hub : ils vivent dans ce navigateur. Posés sans le dire à côté de réglages
      qui valent pour tout le monde, ils laisseraient croire qu'on change la
      langue de tous les comptes — d'où le cadre et la ligne qui les nomment.
    -->
    <section class="card lp-card">
      <h2 class="lp-title">{$_('settings.device_title')}</h2>
      <p class="sub">{$_('settings.device_note')}</p>
      <LangTheme labelled />
    </section>

    <!-- Un fait, pas un réglage : d'où sa place en fin d'onglet. -->
    <section class="card lp-card">
      <h2 class="lp-title">{$_('settings.hub_version_title')}</h2>
      <p class="ro lp-mono">{$_('settings.hub_version', { values: { version: hubVersion } })}</p>
    </section>
  </div>
  {:else if tab === 'alerts'}
  <div class="cards" role="tabpanel" id="panel-alerts" aria-labelledby="tab-alerts" tabindex="-1">
    <!-- Écran d'origine repris tel quel, sans ses onglets internes : il devient
         le contenu de celui-ci. Les abonnements se lisent dès `viewer`, les
         canaux sont réservés à `admin` — le panneau s'en charge lui-même. -->
    <NotificationsView {onExpired} />
  </div>
  {:else if tab === 'accounts'}
  <div class="cards" role="tabpanel" id="panel-accounts" aria-labelledby="tab-accounts" tabindex="-1">
    <AccountsView {onExpired} />
  </div>
  {:else if tab === 'storage'}
  <div class="cards" role="tabpanel" id="panel-storage" aria-labelledby="tab-storage" tabindex="-1">
    <!--
      Ce qu'il y a dans la base avant l'adresse de la base : on ouvre « Stockage »
      pour savoir si les mesures arrivent et ce qu'elles pèsent. L'org et le
      bucket ne se lisent que si la réponse à ça ne suffit pas.
    -->
    <section class="card lp-card">
      <h2 class="lp-title">{$_('settings.influx_state_title')}</h2>

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
        </dl>
      {:else}
        <p class="muted">{$_('settings.state_unavailable')}{influxError ? ` (${influxError})` : ''}</p>
      {/if}
    </section>

    <section class="card lp-card">
      <h2 class="lp-title">{$_('settings.section_influx')}</h2>
      <!-- Les sondes n'écrivent plus dans Influx : elles passent par le hub.
           Cette adresse n'est donc plus un réglage — c'est celle que le hub
           emprunte lui-même, affichée pour le diagnostic. -->
      <div class="lp-field">
        {$_('settings.influx_url_label')}
        <p class="ro lp-mono">{saved.influx_url}</p>
        <span class="hint">{$_('settings.influx_url_hint')}</span>
      </div>
      <div class="pair">
        <label class="lp-field">
          {$_('settings.org_label')}
          <input
            class="lp-input"
            bind:value={org}
            spellcheck="false"
            autocomplete="off"
            disabled={!$isAdmin}
          />
        </label>
        <label class="lp-field">
          {$_('settings.bucket_label')}
          <input
            class="lp-input"
            bind:value={bucket}
            spellcheck="false"
            autocomplete="off"
            disabled={!$isAdmin}
          />
        </label>
      </div>
    </section>

    <!--
      Le jeton de lecture est ce qui fait sortir les mesures du hub : sa carte
      est distincte de l'état de la base parce qu'on ne vient pas ici pour la
      même raison — voir si Influx va bien, ou brancher un Grafana dessus.
      Réservée à `admin` (contrat § 11), donc pas rendue pour les autres.
    -->
    {#if $isAdmin}
    <section class="card lp-card">
      <h2 class="lp-title">{$_('settings.tokens_title')}</h2>
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
                <td class="lp-mono id">{t.auth_id}</td>
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
    </section>
    {/if}

    <!--
      La conservation appartient bien à InfluxDB — c'est la rétention du bucket,
      appliquée par la base. Mais c'est aussi le seul réglage de cet écran qui
      DÉTRUIT des mesures : il est donc renvoyé tout en bas, derrière une
      séparation nommée, pour qu'on ne le croise pas en venant corriger une URL.
      Isolé sans être exilé : au même endroit que ce qu'il règle, jamais sur le
      chemin de ce qu'on est venu faire.
    -->
    <div class="zone">
      <div class="zone-head">
        <span class="zone-title">{$_('settings.destructive_zone')}</span>
        <span class="zone-rule" aria-hidden="true"></span>
      </div>

      <section class="card lp-card danger-frame" class:risky={isShrink}>
        <h2 class="lp-title">{$_('settings.section_retention')}</h2>
        <p class="sub">{$_('settings.retention_lead')}</p>

        <label class="lp-field">
          {$_('settings.retention_label')}
          <span class="unit-row">
            <input
              class="lp-input num"
              type="number"
              min="0"
              step="1"
              bind:value={retention}
              disabled={!$isAdmin}
            />
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
  </div>
  {/if}

  <!-- Le bouton n'appartient qu'aux onglets qui portent des champs du hub. Sur
       les autres, il ne s'affiche que s'il reste une modification en attente
       ailleurs — sinon il disparaîtrait avec du travail non enregistré. -->
  {#if $isAdmin && (dirty || savable)}
    <div class="save-row">
      {#if fieldError}<p class="err" role="alert">{fieldError}</p>{/if}
      {#if notice}<p class="ok" role="status">{notice}</p>{/if}
      <button class="lp-btn primary" onclick={onSave} disabled={busy || !dirty}>
        {busy ? $_('settings.saving_all') : $_('settings.save_all')}
      </button>
    </div>
  {/if}

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

  /* Onglets soulignés plutôt qu'en pastilles : le trait actif prolonge la ligne
     qui sépare la barre du contenu, donc l'onglet et son panneau se lisent
     comme une seule pièce. */
  .tabs {
    display: flex;
    gap: 2px;
    flex-wrap: wrap;
    border-bottom: 1px solid var(--ep-border);
    margin-bottom: 14px;
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
  /* Sous 520 px, la rangée occupe sa ligne à elle seule : on lui rend de la
     place en resserrant les flancs plutôt que de la laisser se replier. Un
     onglet passé à la ligne du dessous se lit comme s'il appartenait à une
     autre famille — c'est précisément ce qu'un regroupement doit éviter. Le
     pouce n'y perd rien, la hauteur de cible passe de 32 à 36 px. */
  @media (max-width: 520px) {
    .tab {
      padding: 10px 8px;
    }
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

  .readonly {
    font-size: 12px;
    color: var(--ep-text-secondary);
    border-left: 2px solid var(--ep-border);
    padding-left: 10px;
    margin: 0 0 12px;
    max-width: 74ch;
  }

  .cards {
    display: flex;
    flex-direction: column;
    gap: 12px;
    /* Pas de colonne étroite : l'écran a la place, autant s'en servir. Les
       champs gardent leur propre largeur de confort — un champ de saisie de
       1400 px est illisible, ce n'est pas la carte qui doit la contraindre. */
    width: 100%;
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

  /* Séparation nommée plutôt qu'un simple blanc : un espace se franchit sans
     s'en apercevoir, un intitulé « Réglage destructif » se lit avant le champ
     qu'il annonce. C'est la seule frontière de l'écran qui doit se remarquer. */
  .zone {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-top: 10px;
  }
  .zone-head {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .zone-title {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.9px;
    font-weight: 700;
    color: var(--ep-danger);
    flex-shrink: 0;
  }
  .zone-rule {
    flex: 1;
    height: 1px;
    background: color-mix(in srgb, var(--ep-danger) 35%, transparent);
  }
  /* Le cadre reste rouge en permanence, pas seulement quand la valeur baisse :
     ce qui est dangereux, c'est le champ, pas l'instant où on le modifie. */
  .card.danger-frame {
    border-left: 3px solid color-mix(in srgb, var(--ep-danger) 55%, var(--ep-border));
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

  /* Le bouton s'aligne sur le bord droit des cartes qu'il enregistre. Une
     largeur maximale héritée d'une mise en page en colonne étroite le laissait
     flotter au milieu du vide sur grand écran, loin de ce sur quoi il agit. */
  .save-row {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    margin-top: 14px;
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
