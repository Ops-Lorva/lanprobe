<script lang="ts">
  import { onMount } from 'svelte';
  import { _ } from 'svelte-i18n';
  import {
    api,
    ApiError,
    type ChannelOutcome,
    type NotifyStatus,
    type ProbeSubscription,
    type SmtpPayload,
    type SmtpSecurity,
    type Subscriptions,
    type WebhookTemplate,
  } from '$lib/api';
  import { isAdmin } from '$lib/session';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import Modal from '$lib/components/Modal.svelte';

  const { onExpired } = $props<{ onExpired: () => void }>();

  /**
   * Un 401 renvoie à la connexion, un 403 non : les canaux sont réservés à
   * `admin` et les abonnements s'écrivent à partir d'`operator`. Se reconnecter
   * avec les mêmes identifiants donnerait exactement le même refus.
   */
  function handle(e: unknown): string {
    if (e instanceof ApiError && e.isExpired) {
      onExpired();
      return '';
    }
    return e instanceof ApiError ? e.message : String(e);
  }

  // Les abonnements s'ouvrent en premier : c'est le seul panneau que tout le
  // monde peut lire (`viewer`), et l'ouvrir d'office évite qu'un observateur
  // déclenche un refus — donc une ligne `access.denied` au journal — juste en
  // cliquant sur « Notifications » dans la barre.
  type Tab = 'subs' | 'channels';
  let tab = $state<Tab>('subs');

  // ══ Abonnements ══════════════════════════════════════════════════════════

  let subs = $state<Subscriptions | null>(null);
  let subsLoading = $state(true);
  let subsError = $state('');
  let subsBusy = $state('');

  async function loadSubs() {
    subsLoading = true;
    subsError = '';
    try {
      subs = await api.subscriptions();
    } catch (e) {
      subs = null;
      subsError = handle(e);
    } finally {
      subsLoading = false;
    }
  }

  /**
   * Après chaque écriture on relit tout.
   *
   * ⚠️ L'héritage est résolu par le hub : `enabled` est ce qui s'applique,
   * `exception` ce qui est réglé sur la sonde. Le recalculer en JS après un clic
   * serait la deuxième implémentation de la règle, donc l'endroit exact où les
   * deux se mettraient à diverger — activer un site montrerait ses sondes
   * activées ici et pas là-bas.
   */
  async function setSub(
    scope: 'site' | 'probe',
    id: string,
    enabled: boolean | null,
    key: string,
  ) {
    subsBusy = key;
    subsError = '';
    let refused = '';
    try {
      await api.setSubscription(scope, id, enabled);
    } catch (e) {
      refused = handle(e);
    }
    // On relit même après un refus. Le navigateur a déjà déplacé la liste
    // déroulante, lui : sans relecture, elle resterait sur un choix que le hub
    // n'a pas retenu — et à côté d'une colonne « effectif » qui, elle, dit la
    // vérité. Un contrôle qui ment est pire qu'un refus.
    await loadSubs();
    if (refused) subsError = refused;
    subsBusy = '';
  }

  type Group = { site_id: string; name: string; enabled: boolean; probes: ProbeSubscription[] };

  const groups = $derived.by<Group[]>(() => {
    if (!subs) return [];
    const out: Group[] = subs.sites.map((s) => ({
      site_id: s.site_id,
      name: s.name,
      enabled: s.enabled,
      probes: subs!.probes.filter((p) => p.site_id === s.site_id),
    }));
    return out;
  });

  const totalProbes = $derived(subs?.probes.length ?? 0);

  /** `null` → « suit son site », `true` → toujours, `false` → jamais. */
  function excToValue(e: boolean | null): string {
    return e == null ? 'inherit' : e ? 'on' : 'off';
  }
  function valueToExc(v: string): boolean | null {
    return v === 'inherit' ? null : v === 'on';
  }

  // ══ Canaux ═══════════════════════════════════════════════════════════════

  /** Une écriture en cours, quelle que soit la carte : un seul verrou suffit. */
  let busy = $state(false);

  let status = $state<NotifyStatus | null>(null);
  let chLoading = $state(false);
  let chLoaded = $state(false);
  let chError = $state('');
  let chDenied = $state('');

  async function loadChannels() {
    if (chLoading) return;
    chLoading = true;
    chError = '';
    chDenied = '';
    try {
      const s = await api.notifications();
      applyStatus(s);
      chLoaded = true;
    } catch (e) {
      const msg = handle(e);
      if (e instanceof ApiError && e.isForbidden) {
        chDenied = msg;
        // Le panneau est fermé pour de bon : on repart sur celui qui sert.
        tab = 'subs';
      } else {
        chError = msg;
      }
    } finally {
      chLoading = false;
    }
  }

  function openChannels() {
    tab = 'channels';
    if (!chLoaded) void loadChannels();
  }

  /** Flèches gauche/droite entre onglets — même comportement que Réglages. */
  function onTabKey(e: KeyboardEvent) {
    if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
    e.preventDefault();
    if (tab === 'subs') {
      if ($isAdmin) openChannels();
    } else {
      tab = 'subs';
    }
    (e.currentTarget as HTMLElement).querySelector<HTMLElement>(`[data-tab="${tab}"]`)?.focus();
  }

  // ── Délai avant alerte ───────────────────────────────────────────────────
  // Il vit dans `settings` et part par `PUT /api/settings`, mais il se règle
  // ici : c'est le paramètre qui décide si un portable qu'on referme réveille
  // quelqu'un. Le poser dans Réglages l'aurait séparé de ce qu'il gouverne.
  let delay = $state('300');
  let delaySaved = $state(300);
  let delayError = $state('');
  let delayNotice = $state('');
  const delayNum = $derived(Number.parseInt(delay, 10));
  // L'équivalent en minutes, seulement quand il tombe juste : « 5 min » aide,
  // « 5 min 17 s » ajoute une deuxième lecture du même nombre.
  const delayHuman = $derived(
    Number.isInteger(delayNum) && delayNum >= 60 && delayNum % 60 === 0
      ? $_('notify.delay_minutes', { values: { n: delayNum / 60 } })
      : '',
  );

  async function saveDelay() {
    delayError = '';
    delayNotice = '';
    if (!Number.isInteger(delayNum) || delayNum <= 0) {
      delayError = $_('notify.delay_invalid');
      return;
    }
    busy = true;
    try {
      await api.saveSettings({ notify_delay_secs: delayNum });
      delaySaved = delayNum;
      delayNotice = $_('notify.delay_saved');
    } catch (e) {
      delayError = handle(e);
    } finally {
      busy = false;
    }
  }

  // ── Webhook ──────────────────────────────────────────────────────────────
  let hookTemplate = $state<WebhookTemplate>('generic');
  let hookUrl = $state('');
  let hookError = $state('');
  let hookNotice = $state('');

  // ── SMTP ─────────────────────────────────────────────────────────────────
  let host = $state('');
  let port = $state('587');
  let security = $state<SmtpSecurity>('starttls');
  let smtpUser = $state('');
  let smtpPassword = $state('');
  /** Coché = on envoie `password: null`, c'est-à-dire « retire-le ». */
  let dropPassword = $state(false);
  let mailFrom = $state('');
  let mailTo = $state('');
  let smtpError = $state('');
  let smtpNotice = $state('');

  function applyStatus(s: NotifyStatus) {
    status = s;
    delaySaved = s.delay_secs;
    delay = String(s.delay_secs);
    hookTemplate = s.webhook.template ?? 'generic';
    hookUrl = '';
    if (s.smtp.configured) {
      host = s.smtp.host ?? '';
      port = String(s.smtp.port ?? 587);
      security = s.smtp.security ?? 'starttls';
      smtpUser = s.smtp.username ?? '';
      mailFrom = s.smtp.from ?? '';
      mailTo = (s.smtp.to ?? []).join('\n');
    }
    smtpPassword = '';
    dropPassword = false;
  }

  async function saveWebhook() {
    hookError = '';
    hookNotice = '';
    busy = true;
    try {
      // ⚠️ `url` n'est envoyée QUE si l'utilisateur en a saisi une. Champ
      // absent = le hub garde l'URL scellée ; champ vide l'aurait remplacée.
      // Corriger le modèle Slack → Discord ne doit pas effacer l'adresse que
      // l'interface n'a jamais vue.
      const body: { template: WebhookTemplate; url?: string } = { template: hookTemplate };
      if (hookUrl.trim()) body.url = hookUrl.trim();
      applyStatus(await api.saveWebhook(body));
      hookNotice = $_('notify.saved');
    } catch (e) {
      hookError = handle(e);
    } finally {
      busy = false;
    }
  }

  async function saveSmtp() {
    smtpError = '';
    smtpNotice = '';
    const recipients = mailTo
      .split(/[\n,;]/)
      .map((a) => a.trim())
      .filter(Boolean);
    const portNum = Number.parseInt(port, 10);
    const payload: SmtpPayload = {
      host: host.trim(),
      port: Number.isInteger(portNum) ? portNum : 0,
      security,
      username: smtpUser.trim() || null,
      from: mailFrom.trim(),
      to: recipients,
    };
    // ⚠️ Les trois cas ne sont pas interchangeables :
    //   champ omis  → le hub garde le mot de passe déjà scellé ;
    //   `null`      → il le retire ;
    //   une chaîne  → il la remplace.
    // Le formulaire ne réaffiche jamais le mot de passe (l'API ne le rend pas),
    // donc corriger un port sans toucher au champ DOIT omettre la clé.
    if (dropPassword) payload.password = null;
    else if (smtpPassword !== '') payload.password = smtpPassword;

    busy = true;
    try {
      applyStatus(await api.saveSmtp(payload));
      smtpNotice = $_('notify.saved');
    } catch (e) {
      smtpError = handle(e);
    } finally {
      busy = false;
    }
  }

  // ── Dé-configuration ─────────────────────────────────────────────────────
  let clearing = $state<'webhook' | 'smtp' | null>(null);
  let clearError = $state('');

  async function confirmClear() {
    if (!clearing) return;
    clearError = '';
    busy = true;
    try {
      applyStatus(clearing === 'webhook' ? await api.clearWebhook() : await api.clearSmtp());
      clearing = null;
    } catch (e) {
      clearError = handle(e);
    } finally {
      busy = false;
    }
  }

  // ── Test ─────────────────────────────────────────────────────────────────
  let outcomes = $state<ChannelOutcome[] | null>(null);
  let testing = $state(false);
  let testError = $state('');

  async function runTest() {
    testing = true;
    testError = '';
    outcomes = null;
    try {
      outcomes = (await api.testNotifications())?.channels ?? [];
    } catch (e) {
      testError = handle(e);
    } finally {
      testing = false;
    }
  }

  onMount(loadSubs);
</script>

<header class="head">
  <h1>{$_('notify.title')}</h1>
</header>

<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
<div
  class="tabs"
  role="tablist"
  tabindex="-1"
  aria-label={$_('notify.tabs_aria')}
  onkeydown={onTabKey}
>
  <button
    class="tab"
    class:on={tab === 'subs'}
    role="tab"
    data-tab="subs"
    id="tab-subs"
    aria-selected={tab === 'subs'}
    aria-controls="panel-subs"
    tabindex={tab === 'subs' ? 0 : -1}
    onclick={() => (tab = 'subs')}
  >
    {$_('notify.tab_subs')}
  </button>
  <!-- Réservé à `admin` (contrat § 11) : le rôle est connu, on ne propose pas
       un onglet dont on sait qu'il sera refusé. -->
  {#if $isAdmin}
    <button
      class="tab"
      class:on={tab === 'channels'}
      role="tab"
      data-tab="channels"
      id="tab-channels"
      aria-selected={tab === 'channels'}
      aria-controls="panel-channels"
      tabindex={tab === 'channels' ? 0 : -1}
      onclick={openChannels}
    >
      {$_('notify.tab_channels')}
    </button>
  {/if}
</div>

<!-- Le hub a refusé les canaux : on le dit une fois, au-dessus des onglets, et
     pas au fond d'un panneau qui peut lui-même être vide. -->
{#if chDenied}<p class="err top" role="alert">{chDenied}</p>{/if}

{#if tab === 'subs'}
  <div class="cards" role="tabpanel" id="panel-subs" aria-labelledby="tab-subs" tabindex="-1">
    {#if subsLoading}
      <StateBlock tone="loading" title={$_('common.loading')} />
    {:else if !subs}
      <StateBlock tone="error" title={$_('notify.error_title')} body={subsError}>
        <button class="lp-btn primary" onclick={loadSubs}>{$_('common.retry')}</button>
      </StateBlock>
    {:else if groups.length === 0}
      <StateBlock tone="empty" title={$_('notify.no_site_title')} body={$_('notify.no_site_body')}>
        <a class="lp-btn primary" href="#/">{$_('notify.no_site_cta')}</a>
      </StateBlock>
    {:else}
      {#if subsError}<p class="err" role="alert">{subsError}</p>{/if}

      <p class="lead">{$_('notify.subs_lead', { values: { n: totalProbes } })}</p>

      {#each groups as g (g.site_id)}
        <section class="card lp-card">
          <div class="site-head">
            <h2 class="lp-title">{g.name}</h2>
            <label class="switch">
              <input
                type="checkbox"
                checked={g.enabled}
                disabled={subsBusy === `site:${g.site_id}`}
                onchange={(ev) =>
                  setSub(
                    'site',
                    g.site_id,
                    (ev.currentTarget as HTMLInputElement).checked,
                    `site:${g.site_id}`,
                  )}
              />
              <span>{$_('notify.site_alert')}</span>
            </label>
          </div>

          {#if g.probes.length === 0}
            <p class="muted">{$_('notify.site_no_probe')}</p>
          {:else}
            <div class="probes">
              {#each g.probes as p (p.probe_id)}
                <div class="prow">
                  <span class="pname">{p.name}</span>

                  <!--
                    Liste déroulante et non trois boutons : la phrase entière
                    tient dans une liste à n'importe quelle largeur, et sur un
                    site de quarante sondes quarante segments côte à côte
                    feraient un mur. « Comme le site » est l'option par défaut
                    et la première : c'est l'état qu'on veut voir dominer.
                  -->
                  <select
                    class="lp-input sel"
                    value={excToValue(p.exception)}
                    disabled={subsBusy === `probe:${p.probe_id}`}
                    aria-label={$_('notify.probe_rule_for', { values: { name: p.name } })}
                    onchange={(ev) =>
                      setSub(
                        'probe',
                        p.probe_id,
                        valueToExc((ev.currentTarget as HTMLSelectElement).value),
                        `probe:${p.probe_id}`,
                      )}
                  >
                    <option value="inherit">{$_('notify.rule_inherit')}</option>
                    <option value="on">{$_('notify.rule_always')}</option>
                    <option value="off">{$_('notify.rule_never')}</option>
                  </select>

                  <!--
                    Ce qui s'applique VRAIMENT, tel que le hub l'a calculé. Sans
                    cette colonne, « comme le site » n'apprend rien : il faut
                    remonter au réglage du site pour savoir si la sonde alerte.
                  -->
                  <span class="eff" class:on={p.enabled}>
                    {p.enabled ? $_('notify.eff_on') : $_('notify.eff_off')}
                    {#if p.exception == null}
                      <span class="from">{$_('notify.eff_from_site')}</span>
                    {/if}
                  </span>
                </div>
              {/each}
            </div>
          {/if}
        </section>
      {/each}
    {/if}
  </div>
{:else if chLoading && !chLoaded}
  <StateBlock tone="loading" title={$_('common.loading')} />
{:else if chError && !status}
  <StateBlock tone="error" title={$_('notify.error_title')} body={chError}>
    <button class="lp-btn primary" onclick={loadChannels}>{$_('common.retry')}</button>
  </StateBlock>
{:else if status}
  <div class="cards" role="tabpanel" id="panel-channels" aria-labelledby="tab-channels" tabindex="-1">
    <!-- Délai avant alerte -->
    <section class="card lp-card">
      <h2 class="lp-title">{$_('notify.delay_title')}</h2>
      <p class="sub">{$_('notify.delay_sub')}</p>
      <div class="inline">
        <label class="lp-field">
          <span class="lp-sr">{$_('notify.delay_title')}</span>
          <span class="unit-row">
            <input class="lp-input num" type="number" min="1" step="1" bind:value={delay} />
            <span class="unit">{$_('notify.delay_unit')}</span>
            {#if delayHuman}<span class="unit">{delayHuman}</span>{/if}
          </span>
        </label>
        <button
          class="lp-btn primary"
          onclick={saveDelay}
          disabled={busy || delayNum === delaySaved}
        >
          {$_('common.save')}
        </button>
      </div>
      {#if delayError}<p class="err" role="alert">{delayError}</p>{/if}
      {#if delayNotice}<p class="ok" role="status">{delayNotice}</p>{/if}
    </section>

    <!-- Webhook -->
    <section class="card lp-card">
      <div class="ch-head">
        <h2 class="lp-title">{$_('notify.webhook_title')}</h2>
        {#if status.webhook.unreadable}
          <span class="pill unreadable">{$_('notify.state_unreadable')}</span>
        {:else if status.webhook.configured}
          <span class="pill set">{$_('notify.state_configured')}</span>
        {:else}
          <span class="pill unset">{$_('notify.state_unset')}</span>
        {/if}
      </div>

      {#if status.webhook.unreadable}
        <!--
          ⚠️ « Illisible » n'est PAS « jamais réglé ». Une valeur scellée existe,
          la clé qui l'ouvre a disparu — volume restauré sans elle, typiquement.
          Les confondre laisse chercher un oubli de configuration qui n'a jamais
          eu lieu. Ce message garde ses mots : il dit quoi faire.
        -->
        <p class="warn">{$_('notify.unreadable_body')}</p>
      {/if}

      <label class="lp-field">
        {$_('notify.webhook_template')}
        <select class="lp-input sel" bind:value={hookTemplate}>
          <option value="generic">{$_('notify.tpl_generic')}</option>
          <option value="slack">Slack</option>
          <option value="discord">Discord</option>
        </select>
      </label>

      <label class="lp-field">
        {$_('notify.webhook_url')}
        <input
          class="lp-input"
          type="url"
          bind:value={hookUrl}
          spellcheck="false"
          autocomplete="off"
          placeholder={status.webhook.configured
            ? $_('notify.webhook_url_keep')
            : 'https://hooks.slack.com/services/…'}
        />
        <span class="hint">{$_('notify.webhook_url_hint')}</span>
      </label>

      {#if hookError}<p class="err" role="alert">{hookError}</p>{/if}
      {#if hookNotice}<p class="ok" role="status">{hookNotice}</p>{/if}

      <div class="row-btns">
        {#if status.webhook.configured || status.webhook.unreadable}
          <button class="lp-btn danger" onclick={() => (clearing = 'webhook')}>
            {$_('notify.unconfigure')}
          </button>
        {/if}
        <button class="lp-btn primary" onclick={saveWebhook} disabled={busy}>
          {busy ? $_('common.saving') : $_('common.save')}
        </button>
      </div>
    </section>

    <!-- SMTP -->
    <section class="card lp-card">
      <div class="ch-head">
        <h2 class="lp-title">{$_('notify.smtp_title')}</h2>
        {#if status.smtp.unreadable}
          <span class="pill unreadable">{$_('notify.state_unreadable')}</span>
        {:else if status.smtp.configured}
          <span class="pill set">{$_('notify.state_configured')}</span>
        {:else}
          <span class="pill unset">{$_('notify.state_unset')}</span>
        {/if}
      </div>

      {#if status.smtp.unreadable}
        <p class="warn">{$_('notify.unreadable_body')}</p>
      {/if}

      <div class="pair">
        <label class="lp-field grow">
          {$_('notify.smtp_host')}
          <input class="lp-input" bind:value={host} spellcheck="false" autocomplete="off" />
        </label>
        <label class="lp-field">
          {$_('notify.smtp_port')}
          <input class="lp-input num" type="number" min="1" step="1" bind:value={port} />
        </label>
        <label class="lp-field">
          {$_('notify.smtp_security')}
          <select class="lp-input sel" bind:value={security}>
            <option value="starttls">STARTTLS</option>
            <option value="tls">TLS</option>
            <option value="none">{$_('notify.sec_none')}</option>
          </select>
        </label>
      </div>

      <div class="pair">
        <label class="lp-field grow">
          {$_('notify.smtp_user')}
          <input class="lp-input" bind:value={smtpUser} spellcheck="false" autocomplete="off" />
        </label>
        <label class="lp-field grow">
          {$_('notify.smtp_password')}
          <input
            class="lp-input"
            type="password"
            bind:value={smtpPassword}
            disabled={dropPassword}
            autocomplete="new-password"
            placeholder={status.smtp.password_set ? $_('notify.smtp_password_keep') : ''}
          />
          {#if status.smtp.password_set}
            <!--
              ⚠️ Retirer le mot de passe est une action explicite, jamais un
              effet de bord d'un champ laissé vide : l'API ne rend pas le mot de
              passe, donc l'interface ne peut pas le remettre dans le champ, et
              enregistrer une correction de port l'effacerait un jour sur deux.
            -->
            <label class="drop">
              <input type="checkbox" bind:checked={dropPassword} />
              <span>{$_('notify.smtp_password_drop')}</span>
            </label>
          {:else}
            <span class="hint">{$_('notify.smtp_password_hint')}</span>
          {/if}
        </label>
      </div>

      <div class="pair">
        <label class="lp-field grow">
          {$_('notify.smtp_from')}
          <input
            class="lp-input"
            type="email"
            bind:value={mailFrom}
            spellcheck="false"
            autocomplete="off"
          />
        </label>
      </div>

      <label class="lp-field">
        {$_('notify.smtp_to')}
        <textarea class="lp-input area" rows="3" bind:value={mailTo} spellcheck="false"></textarea>
        <span class="hint">{$_('notify.smtp_to_hint')}</span>
      </label>

      {#if smtpError}<p class="err" role="alert">{smtpError}</p>{/if}
      {#if smtpNotice}<p class="ok" role="status">{smtpNotice}</p>{/if}

      <div class="row-btns">
        {#if status.smtp.configured || status.smtp.unreadable}
          <button class="lp-btn danger" onclick={() => (clearing = 'smtp')}>
            {$_('notify.unconfigure')}
          </button>
        {/if}
        <button class="lp-btn primary" onclick={saveSmtp} disabled={busy}>
          {busy ? $_('common.saving') : $_('common.save')}
        </button>
      </div>
    </section>

    <!-- Test -->
    <section class="card lp-card">
      <h2 class="lp-title">{$_('notify.test_title')}</h2>
      <p class="sub">{$_('notify.test_sub')}</p>
      <div>
        <button class="lp-btn accent" onclick={runTest} disabled={testing}>
          {testing ? $_('notify.testing') : $_('notify.test_send')}
        </button>
      </div>

      {#if testError}<p class="err" role="alert">{testError}</p>{/if}

      {#if outcomes}
        <!-- Verdict par canal : un canal en panne n'empêche pas l'autre, et
             c'est le jour où l'un tombe qu'on a besoin de savoir lequel. -->
        <ul class="verdicts">
          {#each outcomes as o (o.channel)}
            <li class:bad={o.attempted && !o.ok} class:good={o.attempted && o.ok}>
              <span class="ch">{$_(`notify.channel_${o.channel}`)}</span>
              {#if !o.attempted}
                <span class="v">{$_('notify.test_skipped')}</span>
              {:else if o.ok}
                <span class="v">{$_('notify.test_ok')}</span>
              {:else}
                <span class="v">{$_('notify.test_failed')}</span>
                {#if o.error}<span class="why">{o.error}</span>{/if}
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  </div>
{/if}

<Modal
  open={clearing !== null}
  title={$_('notify.unconfigure_title')}
  onclose={() => (clearing = null)}
>
  <p>{$_('notify.unconfigure_body')}</p>
  <p class="keeps">{$_('notify.unconfigure_keeps')}</p>
  {#if clearError}<p class="err" role="alert">{clearError}</p>{/if}
  {#snippet footer()}
    <button class="lp-btn" onclick={() => (clearing = null)}>{$_('common.cancel')}</button>
    <button class="lp-btn danger" onclick={confirmClear} disabled={busy}>
      {$_('notify.unconfigure_confirm')}
    </button>
  {/snippet}
</Modal>

<style>
  .head {
    margin-bottom: 14px;
  }
  h1 {
    font-size: 20px;
    font-weight: 800;
    letter-spacing: -0.02em;
    margin: 0;
  }

  /* Même onglets soulignés que Réglages : le trait actif prolonge la ligne qui
     sépare la barre du contenu, donc l'onglet et son panneau se lisent comme
     une seule pièce. */
  .tabs {
    display: flex;
    gap: 2px;
    flex-wrap: wrap;
    border-bottom: 1px solid var(--ep-border);
    margin-bottom: 14px;
  }
  .tab {
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

  .cards {
    display: flex;
    flex-direction: column;
    gap: 12px;
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
    min-width: 0;
  }
  .lead {
    font-size: 12px;
    color: var(--ep-text-secondary);
    margin: 0;
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
    line-height: 1.5;
    max-width: 74ch;
  }
  .muted {
    color: var(--ep-text-muted);
    font-size: 12px;
    margin: 0;
  }

  .site-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }
  .switch {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--ep-text-primary);
    cursor: pointer;
  }
  .switch input {
    accent-color: var(--ep-accent);
    width: 16px;
    height: 16px;
  }

  .probes {
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--ep-border);
  }
  .prow {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 190px 150px;
    align-items: center;
    gap: 10px;
    padding: 8px 0;
    border-bottom: 1px solid var(--ep-border);
  }
  .prow:last-child {
    border-bottom: none;
  }
  .pname {
    font-size: 12.5px;
    font-weight: 500;
    color: var(--ep-text-primary);
    overflow-wrap: anywhere;
  }
  .sel {
    font-family: var(--ep-font-sans);
  }
  .prow .sel {
    min-height: 30px;
    padding: 5px 8px;
    font-size: 12px;
  }
  /* L'état effectif est du texte, pas une couleur seule : « inactif » doit se
     lire, y compris quand la ligne est parcourue en diagonale. */
  .eff {
    display: flex;
    flex-direction: column;
    font-size: 11px;
    color: var(--ep-text-muted);
    text-align: right;
  }
  .eff.on {
    color: var(--ep-success);
    font-weight: 600;
  }
  .from {
    font-size: 10px;
    color: var(--ep-text-dim);
    font-weight: 400;
  }

  .ch-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    flex-wrap: wrap;
  }
  .pill {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.4px;
    text-transform: uppercase;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--ep-border-strong);
    color: var(--ep-text-muted);
    white-space: nowrap;
  }
  .pill.set {
    border-color: color-mix(in srgb, var(--ep-success) 55%, var(--ep-border));
    background: color-mix(in srgb, var(--ep-success) 10%, transparent);
    color: var(--ep-success);
  }
  /* Trois états, trois traitements. « Illisible » emprunte le rouge parce que
     c'est une perte : la valeur existe et ne servira plus. « Non configuré »
     reste gris — ce n'est pas une panne, c'est une étape pas encore faite. */
  .pill.unreadable {
    border-color: var(--ep-danger);
    background: color-mix(in srgb, var(--ep-danger) 10%, transparent);
    color: var(--ep-danger);
  }
  .warn {
    border: 1px solid color-mix(in srgb, var(--ep-danger) 45%, var(--ep-border));
    border-left-width: 3px;
    border-radius: var(--ep-radius-md);
    background: color-mix(in srgb, var(--ep-danger) 7%, transparent);
    padding: 11px 13px;
    margin: 0;
    font-size: 11.5px;
    line-height: 1.6;
    color: var(--ep-text-secondary);
    max-width: 78ch;
  }

  /* Un champ de saisie de 1 200 px est illisible : l'œil perd le début de la
     ligne. La carte prend toute la largeur, ses champs non. */
  .card :global(.lp-field) {
    max-width: 620px;
  }
  .pair {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
    max-width: 900px;
  }
  .grow {
    flex: 1 1 200px;
    min-width: 0;
  }
  .inline {
    display: flex;
    align-items: flex-end;
    gap: 12px;
    flex-wrap: wrap;
  }
  .unit-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .num {
    width: 110px;
    font-variant-numeric: tabular-nums;
  }
  .unit {
    font-size: 12px;
    color: var(--ep-text-muted);
  }
  .area {
    font-family: var(--ep-font-mono);
    resize: vertical;
    min-height: 64px;
  }
  .drop {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 11.5px;
    color: var(--ep-danger);
    cursor: pointer;
  }
  .drop input {
    accent-color: var(--ep-danger);
    width: 14px;
    height: 14px;
  }

  .row-btns {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    flex-wrap: wrap;
    border-top: 1px solid var(--ep-border);
    padding-top: 12px;
  }

  .verdicts {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .verdicts li {
    display: flex;
    align-items: baseline;
    gap: 10px;
    flex-wrap: wrap;
    font-size: 12px;
    padding: 8px 11px;
    border: 1px solid var(--ep-border);
    border-left-width: 3px;
    border-radius: var(--ep-radius-md);
    background: var(--ep-bg-tertiary);
    color: var(--ep-text-muted);
  }
  .verdicts li.good {
    border-left-color: var(--ep-success);
    color: var(--ep-text-secondary);
  }
  .verdicts li.bad {
    border-left-color: var(--ep-danger);
    background: color-mix(in srgb, var(--ep-danger) 7%, transparent);
  }
  .verdicts .ch {
    font-weight: 700;
    color: var(--ep-text-primary);
    min-width: 72px;
  }
  .verdicts li.bad .v {
    color: var(--ep-danger);
    font-weight: 600;
  }
  .verdicts .why {
    font-family: var(--ep-font-mono);
    font-size: 11px;
    overflow-wrap: anywhere;
  }

  .err {
    font-size: 12px;
    color: var(--ep-danger);
    margin: 0;
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
  }
  .err.top {
    margin-bottom: 12px;
  }
  .ok {
    font-size: 12px;
    color: var(--ep-success);
    margin: 0;
  }
  .keeps {
    color: var(--ep-text-primary);
    border-left: 2px solid var(--ep-accent);
    padding-left: 10px;
    margin: 0;
  }

  /* À 390 px la ligne d'une sonde s'empile : le nom sur une ligne, la règle et
     l'état effectif côte à côte en dessous. Une liste déroulante de 190 px et
     un nom de sonde ne tiennent pas sur la même ligne sans écraser l'un des
     deux — et c'est le nom qu'on lit en premier. */
  @media (max-width: 700px) {
    .prow {
      grid-template-columns: minmax(0, 1fr) auto;
      row-gap: 6px;
    }
    .pname {
      grid-column: 1 / -1;
    }
    .eff {
      text-align: right;
    }
  }
</style>
