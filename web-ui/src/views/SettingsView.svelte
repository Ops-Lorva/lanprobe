<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { _, locale } from 'svelte-i18n';
  import {
    api,
    ApiError,
    SETTINGS_DEFAULTS,
    type AdvertiseTest,
    type BackupList,
    type HubSettings,
    type InfluxInfo,
    type Passkey,
    type ReadToken,
  } from '$lib/api';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import CopyLine from '$lib/components/CopyLine.svelte';
  import LangTheme from '$lib/components/LangTheme.svelte';
  import BackupPanel from '$lib/components/BackupPanel.svelte';
  import RestoreCard from '$lib/components/RestoreCard.svelte';
  import AccountsView from './AccountsView.svelte';
  import NotificationsView from './NotificationsView.svelte';
  import { go, route, type SettingsTab } from '$lib/router';
  import { canOperate, identity, isAdmin } from '$lib/session';
  import { dateOnly, humanBytes } from '$lib/time';
  import { passkeysSupported, createCredential, ceremonyError } from '$lib/webauthn';

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
  // Le hub le dit lui-même : un réglage lu au démarrage a changé depuis.
  // C'est la seule source honnête — l'interface, elle, ne sait pas avec quels
  // arguments le processus a été lancé.
  let restartPending = $state(false);
  let retention = $state('0');
  let heartbeat = $state('60');
  let backupEnabled = $state(true);
  let backupInterval = $state('24');
  let backupKeep = $state('7');
  let inventoryDays = $state('0');
  let tlsEnabled = $state(false);
  let trustedProxies = $state('');

  let fieldError = $state('');
  let busy = $state(false);
  let notice = $state('');

  function fill(s: HubSettings) {
    saved = s;
    influxUrl = s.influx_url;
    org = s.influx_org;
    bucket = s.influx_bucket;
    hubPublicUrl = s.hub_public_url ?? '';
    api
      .status()
      .then((st) => {
        hubVersion = st.version || '—';
        restartPending = st.restart_required === true;
      })
      .catch(() => {});
    retention = String(s.retention_days);
    heartbeat = String(s.heartbeat_interval_secs);
    backupEnabled = s.backup_enabled;
    backupInterval = String(s.backup_interval_hours);
    backupKeep = String(s.backup_keep_last);
    inventoryDays = String(s.inventory_days);
    tlsEnabled = s.tls_enabled;
    // ⚠️ `?? ''` et non `s.trusted_proxies` : un hub plus ancien que ce
    // réglage ne le renvoie pas, et `undefined` dans un `<input>` laisserait
    // un champ non contrôlé qui se croit modifié dès l'ouverture.
    trustedProxies = s.trusted_proxies ?? '';
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
    void loadTotp();
    void loadPasskeys();
    void loadInflux();
    // Ces deux-là ont un rôle minimum (contrat § 11) : le test d'URL annoncée
    // demande `operator`, les jetons de lecture `admin`. Les appeler pour tout
    // le monde déposait une ligne `access.denied` au journal à chaque visite
    // d'un observateur — le bruit exact qui masque les refus qui comptent.
    if (get(isAdmin)) {
      void loadTokens();
      void loadBackups();
    }
  });

  // ── Sauvegardes ──────────────────────────────────────────────────────────
  //
  // La liste est tenue ici, et non dans chaque carte : la carte des archives
  // et celle de la restauration lisent la même chose. Deux composants qui
  // interrogeraient chacun `/api/backups` afficheraient deux états différents
  // dès qu'une sauvegarde tombe entre les deux requêtes — et c'est la liste
  // des sauvegardes, l'endroit où se contredire coûte le plus cher.
  let backups = $state<BackupList | null>(null);
  let backupsError = $state('');

  async function loadBackups() {
    try {
      backups = await api.backups();
      backupsError = '';
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      backups = null;
      backupsError = e instanceof ApiError ? e.message : String(e);
    }
  }

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

  // Le test d'URL annoncée a disparu avec la carte qu'il alimentait : les
  // sondes n'écrivent plus dans Influx, elles passent par le hub. L'appel
  // subsistait sans que personne n'en voie jamais le verdict — une requête
  // sortante à chaque ouverture des Réglages, pour rien.

  // ── Enregistrement ───────────────────────────────────────────────────────
  const retentionNum = $derived(Number.parseInt(retention, 10));
  const heartbeatNum = $derived(Number.parseInt(heartbeat, 10));
  const intervalNum = $derived(Number.parseInt(backupInterval, 10));
  const keepNum = $derived(Number.parseInt(backupKeep, 10));
  const inventoryNum = $derived(Number.parseInt(inventoryDays, 10));

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

  /**
   * Même règle pour les archives : `0` vaut « toutes », donc en partir est la
   * réduction la plus destructrice — pas une augmentation. Le hub refuse ce
   * changement en 409 tant que `confirm_data_loss` n'accompagne pas la requête.
   */
  const keepShrink = $derived(
    Number.isInteger(keepNum) &&
      keepNum >= 0 &&
      ((saved.backup_keep_last === 0 && keepNum > 0) ||
        (saved.backup_keep_last > 0 && keepNum > 0 && keepNum < saved.backup_keep_last)),
  );

  /**
   * Troisième réduction possible de cet écran, et exactement la même règle :
   * `0` vaut « illimité », donc en partir efface tout ce qui dépasse.
   */
  const inventoryShrink = $derived(
    Number.isInteger(inventoryNum) &&
      inventoryNum >= 0 &&
      ((saved.inventory_days === 0 && inventoryNum > 0) ||
        (saved.inventory_days > 0 && inventoryNum > 0 && inventoryNum < saved.inventory_days)),
  );

  function inventoryLabel(days: number) {
    return days === 0
      ? $_('settings.retention_unlimited_label')
      : $_('settings.retention_days_label', { values: { n: days } });
  }

  function keepLabel(n: number) {
    return n === 0 ? $_('backup.keep_all_label') : $_('backup.keep_n_label', { values: { n } });
  }

  /** Les deux réductions partent dans le même PUT, donc dans la même confirmation. */
  const dataLoss = $derived(isShrink || keepShrink || inventoryShrink);
  const bothLosses = $derived(isShrink && keepShrink);

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
    if (backupEnabled !== saved.backup_enabled) p.backup_enabled = backupEnabled;
    if (intervalNum !== saved.backup_interval_hours) p.backup_interval_hours = intervalNum;
    if (keepNum !== saved.backup_keep_last) p.backup_keep_last = keepNum;
    if (inventoryNum !== saved.inventory_days) p.inventory_days = inventoryNum;
    if (tlsEnabled !== saved.tls_enabled) p.tls_enabled = tlsEnabled;
    if (trustedProxies.trim() !== (saved.trusted_proxies ?? ''))
      p.trusted_proxies = trustedProxies.trim();
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
  // ── Mon compte ───────────────────────────────────────────────────────────
  let pwCurrent = $state('');
  let pwNext = $state('');
  let pwConfirm = $state('');
  let pwBusy = $state(false);
  let pwError = $state('');
  let pwDone = $state(false);

  // ── Second facteur (contrat § 19) ────────────────────────────────────────
  let totpEnabled = $state(false);
  let totpPending = $state(false);
  let totpSecret = $state('');
  let totpQr = $state('');
  let totpCode = $state('');
  let totpBusy = $state(false);
  let totpError = $state('');
  let totpOffPassword = $state('');
  let totpOffOpen = $state(false);

  async function loadTotp() {
    try {
      const st = await api.totpStatus();
      totpEnabled = st.enabled;
      totpPending = st.pending;
    } catch {
      /* le reste de l'écran n'a pas à tomber pour autant */
    }
  }

  async function startTotp() {
    totpBusy = true;
    totpError = '';
    try {
      const started = await api.totpStart();
      totpSecret = started.secret;
      totpQr = started.qr_svg;
      totpCode = '';
    } catch (e) {
      totpError = e instanceof ApiError ? e.message : String(e);
    } finally {
      totpBusy = false;
    }
  }

  async function confirmTotp() {
    totpBusy = true;
    totpError = '';
    try {
      await api.totpConfirm(totpCode.trim());
      // Le secret disparaît de l'écran dès qu'il a servi : il n'a plus aucune
      // raison d'être lisible par-dessus l'épaule.
      totpSecret = '';
      totpQr = '';
      totpCode = '';
      await loadTotp();
    } catch (e) {
      totpError = e instanceof ApiError ? e.message : String(e);
    } finally {
      totpBusy = false;
    }
  }

  async function disableTotp() {
    totpBusy = true;
    totpError = '';
    try {
      await api.totpDisable(totpOffPassword);
      totpOffPassword = '';
      totpOffOpen = false;
      await loadTotp();
    } catch (e) {
      totpError = e instanceof ApiError ? e.message : String(e);
    } finally {
      totpBusy = false;
    }
  }

  // ── Clés d'accès (contrat § 19) ──────────────────────────────────────────
  let passkeys = $state<Passkey[]>([]);
  let pkSecure = $state(false);
  let pkRpId = $state('');
  let pkLabel = $state('');
  let pkBusy = $state(false);
  let pkError = $state('');

  /**
   * Deux conditions, et pas une : le hub doit servir sur une origine sûre, ET
   * le navigateur doit exposer l'API. Un navigateur ancien sur un hub en HTTPS
   * échouerait sinon au clic, sans rien expliquer.
   */
  const pkUsable = $derived(pkSecure && passkeysSupported());

  async function loadPasskeys() {
    try {
      const r = await api.passkeys();
      passkeys = r.passkeys;
      pkSecure = r.secure_context;
      pkRpId = r.rp_id;
    } catch {
      /* le reste de l'écran n'a pas à tomber pour autant */
    }
  }

  async function addPasskey() {
    pkBusy = true;
    pkError = '';
    try {
      const challenge = await api.passkeyStart();
      const credential = await createCredential(challenge as never);
      await api.passkeyFinish(credential, pkLabel.trim());
      pkLabel = '';
      await loadPasskeys();
    } catch (e) {
      pkError = e instanceof ApiError ? e.message : ceremonyError(e, (k) => $_(k));
    } finally {
      pkBusy = false;
    }
  }

  async function removePasskey(id: string) {
    pkBusy = true;
    pkError = '';
    try {
      await api.passkeyDelete(id);
      await loadPasskeys();
    } catch (e) {
      pkError = e instanceof ApiError ? e.message : String(e);
    } finally {
      pkBusy = false;
    }
  }

  const pwReady = $derived(
    pwCurrent !== '' && pwNext.length >= 8 && pwNext === pwConfirm && pwNext !== pwCurrent,
  );

  async function changePassword() {
    pwBusy = true;
    pwError = '';
    pwDone = false;
    try {
      await api.changeOwnPassword(pwCurrent, pwNext);
      // Vidés tout de suite : trois champs qui gardent un mot de passe en
      // clair après coup, c'est un post-it sur l'écran.
      pwCurrent = '';
      pwNext = '';
      pwConfirm = '';
      pwDone = true;
    } catch (e) {
      pwError = e instanceof ApiError ? e.message : String(e);
    } finally {
      pwBusy = false;
    }
  }

  const ALL_TABS: { id: Tab; key: string; admin?: true }[] = [
    // En tête : c'est le seul onglet que TOUS les rôles peuvent utiliser, et
    // celui qu'un lecteur vient chercher. Le reléguer en fin de rangée le
    // ferait chercher parmi des onglets qui ne le concernent pas.
    { id: 'account', key: 'settings.tab_account' },
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
    // « Mon compte » n'a rien de différé : un mot de passe s'enregistre par son
    // propre bouton, ou pas du tout. Le laisser marquer « non enregistré »
    // ferait chercher un bouton « Enregistrer les réglages » qui ne le
    // concerne pas.
    account: false,
    // Langue et thème s'appliquent au clic et ne partent jamais au hub : rien à
    // y enregistrer de leur fait.
    general:
      'hub_public_url' in patch ||
      'heartbeat_interval_secs' in patch ||
      'trusted_proxies' in patch,
    storage:
      'influx_url' in patch ||
      'influx_org' in patch ||
      'influx_bucket' in patch ||
      'retention_days' in patch ||
      'backup_enabled' in patch ||
      'backup_interval_hours' in patch ||
      'backup_keep_last' in patch,
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
    // Le hub refuse une cadence nulle ou négative. On le dit ici plutôt que de
    // laisser partir la requête : le message serait le même, avec un aller-
    // retour et un champ qu'on ne saurait pas retrouver.
    if (!Number.isInteger(intervalNum) || intervalNum < 1)
      return { msg: $_('backup.invalid_interval'), tab: 'storage' };
    if (!Number.isInteger(keepNum) || keepNum < 0)
      return { msg: $_('backup.invalid_keep'), tab: 'storage' };
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
    if (dataLoss) {
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
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      fieldError = e instanceof ApiError ? e.message : String(e);
      shrinkOpen = false;
      // ⚠️ Le hub écrit les réglages un par un et s'arrête au premier refus :
      // ceux qui précédaient sont DÉJÀ en base. On relit donc la RÉFÉRENCE,
      // sans toucher au brouillon : les champs déjà passés sortent d'eux-mêmes
      // du prochain envoi, et ce que la personne vient de taper reste à
      // l'écran. Recharger tout l'écran effacerait sa saisie au moment précis
      // où elle a besoin de la corriger.
      try {
        saved = await api.settings();
      } catch {
        /* la référence attendra : l'erreur d'enregistrement prime */
      }
    } finally {
      busy = false;
    }
    // La rétention des archives a pu en retirer : la liste doit suivre.
    if (get(isAdmin)) void loadBackups();
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

  {#if tab === 'account'}
  <div class="cards" role="tabpanel" id="panel-account" aria-labelledby="tab-account" tabindex="-1">
    <section class="lp-card block">
      <h2 class="lp-title">{$_('account.identity')}</h2>
      <dl class="idlist">
        <div><dt>{$_('accounts.username')}</dt><dd class="lp-mono">{$identity?.username ?? '—'}</dd></div>
        <div>
          <dt>{$_('accounts.role')}</dt>
          <dd>{$identity ? $_(`accounts.role_${$identity.role}`) : '—'}</dd>
        </div>
      </dl>
      <p class="hint">{$_('account.role_hint')}</p>
    </section>

    <section class="lp-card block">
      <h2 class="lp-title">{$_('account.password')}</h2>
      <!-- ⚠️ L'ancien mot de passe est exigé par le hub, pas seulement demandé
           ici : une session volée suffirait sinon à s'approprier le compte
           définitivement, sans aucun chemin de retour pour son titulaire. -->
      <p class="hint">{$_('account.password_hint')}</p>
      <label class="lp-field">
        {$_('account.current')}
        <input class="lp-input" type="password" bind:value={pwCurrent} autocomplete="current-password" />
      </label>
      <label class="lp-field">
        {$_('account.next')}
        <input class="lp-input" type="password" bind:value={pwNext} autocomplete="new-password" />
      </label>
      <label class="lp-field">
        {$_('account.confirm')}
        <input class="lp-input" type="password" bind:value={pwConfirm} autocomplete="new-password" />
      </label>
      <div class="pwrow">
        <button class="lp-btn primary" onclick={changePassword} disabled={pwBusy || !pwReady}>
          {pwBusy ? $_('common.saving') : $_('account.change')}
        </button>
        {#if pwError}<span class="pwerr">{pwError}</span>{/if}
        {#if pwDone}<span class="pwok">{$_('account.changed')}</span>{/if}
      </div>
    </section>

    <section class="lp-card block">
      <h2 class="lp-title">{$_('account.second_factor')}</h2>

      {#if totpEnabled}
        <p class="ok" role="status">{$_('account.totp_on')}</p>
        <p class="hint">{$_('account.totp_lost_device')}</p>
        {#if totpOffOpen}
          <!-- Le mot de passe est exigé par le hub, pas seulement demandé
               ici : sans lui, une session volée suffirait à retirer le
               second facteur, c'est-à-dire à annuler ce contre quoi il
               protège. -->
          <label class="lp-field">
            {$_('account.current_password')}
            <input
              class="lp-input"
              type="password"
              bind:value={totpOffPassword}
              autocomplete="current-password"
            />
          </label>
          <div class="row-btns">
            <button class="lp-btn" onclick={() => (totpOffOpen = false)}>
              {$_('common.cancel')}
            </button>
            <button
              class="lp-btn danger"
              onclick={disableTotp}
              disabled={totpBusy || totpOffPassword === ''}
            >
              {$_('account.totp_disable')}
            </button>
          </div>
        {:else}
          <div class="row-btns">
            <button class="lp-btn danger" onclick={() => (totpOffOpen = true)}>
              {$_('account.totp_disable')}
            </button>
          </div>
        {/if}
      {:else if totpSecret}
        <p class="sub">{$_('account.totp_scan')}</p>
        <!-- QR rendu par le hub, pas par une bibliothèque JavaScript : le
             secret ne traverse aucune dépendance de plus, et la page reste
             lisible sur un réseau fermé, sans CDN. -->
        <div class="qr">{@html totpQr}</div>
        <p class="hint">{$_('account.totp_manual')}</p>
        <p class="secret lp-mono">{totpSecret}</p>

        <label class="lp-field">
          {$_('account.totp_code')}
          <input
            class="lp-input code"
            type="text"
            bind:value={totpCode}
            inputmode="numeric"
            autocomplete="one-time-code"
            maxlength="6"
            placeholder="000000"
          />
          <span class="hint">{$_('account.totp_confirm_hint')}</span>
        </label>

        {#if totpError}<p class="err" role="alert">{totpError}</p>{/if}
        <div class="row-btns">
          <button
            class="lp-btn primary"
            onclick={confirmTotp}
            disabled={totpBusy || totpCode.trim().length !== 6}
          >
            {$_('account.totp_activate')}
          </button>
        </div>
      {:else}
        <p class="sub">{$_('account.totp_lead')}</p>
        {#if totpPending}
          <!-- Un secret existe, personne n'a prouvé qu'il fonctionne : le dire
               évite de croire la 2FA active alors que la connexion ne la
               demande pas. -->
          <p class="hint">{$_('account.totp_pending')}</p>
        {/if}
        {#if totpError}<p class="err" role="alert">{totpError}</p>{/if}
        <div class="row-btns">
          <button class="lp-btn primary" onclick={startTotp} disabled={totpBusy}>
            {$_('account.totp_enable')}
          </button>
        </div>
      {/if}

    </section>

    <section class="lp-card block">
      <h2 class="lp-title">{$_('account.passkeys')}</h2>
      <p class="sub">{$_('account.pk_lead')}</p>

      {#if passkeys.length > 0}
        <div class="pk-list">
          {#each passkeys as k (k.id)}
            <div class="pk-row">
              <span class="pk-name">{k.label}</span>
              <span class="pk-when">
                {k.last_used_at
                  ? $_('account.pk_last_used', {
                      values: { date: dateOnly(k.last_used_at, lang) },
                    })
                  : $_('account.pk_never_used')}
              </span>
              <button class="lp-btn danger sm" disabled={pkBusy} onclick={() => removePasskey(k.id)}>
                {$_('account.pk_remove')}
              </button>
            </div>
          {/each}
        </div>
      {/if}

      {#if pkUsable}
        <label class="lp-field">
          {$_('account.pk_label')}
          <input
            class="lp-input"
            bind:value={pkLabel}
            placeholder={$_('account.pk_label_placeholder')}
            maxlength="40"
          />
          <!-- Sans nom, « en révoquer une » n'a aucun sens dès qu'on en a
               deux : on ne saurait pas laquelle est le téléphone perdu. -->
          <span class="hint">{$_('account.pk_label_hint')}</span>
        </label>
        {#if pkError}<p class="err" role="alert">{pkError}</p>{/if}
        <div class="row-btns">
          <button class="lp-btn primary" onclick={addPasskey} disabled={pkBusy}>
            {pkBusy ? $_('account.pk_waiting') : $_('account.pk_add')}
          </button>
        </div>
      {:else}
        <!-- ⚠️ Le bouton n'est pas grisé, il est ABSENT, et la raison est
             écrite. Le navigateur refuserait l'API sans message exploitable,
             et un bouton qui ne fait rien laisse chercher la panne ailleurs. -->
        <p class="warn-inline">{$_('account.passkey_warning')}</p>
        {#if pkRpId}
          <p class="hint">{$_('account.pk_rp', { values: { host: pkRpId } })}</p>
        {/if}
      {/if}

      {#if pkUsable && pkRpId}
        <p class="hint">{$_('account.pk_rp_bound', { values: { host: pkRpId } })}</p>
      {/if}
    </section>
  </div>
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

    <!--
      Juste sous l'adresse du hub : c'est la même question — « par où arrive-t-on
      ici ? ». ⚠️ Champ vide = `X-Forwarded-For` ignoré, et c'est le réglage
      sûr, pas un oubli : l'en-tête est écrit par le client, l'accepter sans
      liste laisserait n'importe qui se donner l'adresse qu'il veut. L'aide le
      dit en toutes lettres, sinon on remplit le champ « au cas où ».
    -->
    <section class="card lp-card">
      <h2 class="lp-title">{$_('settings.proxies_title')}</h2>
      <label class="lp-field">
        {$_('settings.proxies_label')}
        <input
          class="lp-input"
          bind:value={trustedProxies}
          placeholder="10.0.0.0/8, 172.16.0.0/12"
          spellcheck="false"
          autocomplete="off"
          disabled={!$isAdmin}
        />
        <span class="hint">{$_('settings.proxies_hint')}</span>
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

    <!--
      ⚠️ Le seul réglage de cet onglet qui puisse rendre le hub injoignable.
      Il ne détruit rien — d'où sa place ici et non dans la zone rouge — mais
      il change l'adresse par laquelle tout le monde arrive, sondes comprises.
      Les conséquences sont énumérées AVANT le clic, pas après : découvrir
      qu'on a coupé ses sondes en rechargeant une page qui ne répond plus est
      exactement ce qu'il faut éviter.
    -->
    <section class="card lp-card">
      <h2 class="lp-title">{$_('settings.tls_title')}</h2>
      <p class="sub">{$_('settings.tls_lead')}</p>

      <label class="switch">
        <input type="checkbox" bind:checked={tlsEnabled} disabled={!$isAdmin} />
        <span>{$_('settings.tls_label')}</span>
      </label>

      {#if tlsEnabled !== saved.tls_enabled}
        <div class="warn" role="status">
          <strong>{$_('settings.tls_warn_title')}</strong>
          <ul class="tls-list">
            <li>{$_('settings.tls_warn_url', { values: { scheme: tlsEnabled ? 'https://' : 'http://' } })}</li>
            <li>{$_('settings.tls_warn_probes')}</li>
            <li>{$_('settings.tls_warn_selfsigned')}</li>
            <li>{$_('settings.tls_warn_restart')}</li>
          </ul>
        </div>
      {:else if restartPending}
        <!-- Enregistré ≠ appliqué. Sans cette ligne, l'écran affiche le
             réglage voulu et le hub sert encore l'autre : on recharge en
             `https://`, on tombe sur rien, et on croit avoir cassé le hub. -->
        <p class="cur pending">{$_('settings.tls_restart_pending')}</p>
      {:else if saved.tls_enabled}
        <p class="cur">{$_('settings.tls_current_on')}</p>
      {:else}
        <p class="cur">{$_('settings.tls_current_off')}</p>
      {/if}
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

    <!--
      La sauvegarde vient après les jetons parce qu'elle répond à une question
      plus rare : « qu'est-ce qui me reste si cette machine disparaît ? ». On
      ouvre « Stockage » d'abord pour voir si les mesures arrivent ; on y
      revient, plus tard et moins souvent, pour vérifier le filet. Réservée à
      `admin` comme les routes qu'elle appelle.
    -->
    <BackupPanel
      list={backups}
      listError={backupsError}
      onReload={loadBackups}
      {onExpired}
      bind:enabled={backupEnabled}
      bind:intervalHours={backupInterval}
      bind:keepLast={backupKeep}
    />
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

      <!--
        La restauration ouvre la zone : c'est la plus lourde des deux. Elle
        remplace tout le hub, là où la rétention ne touche qu'aux mesures. Et
        elle est ici, sous la même barre rouge, plutôt qu'à côté de la liste
        des archives : la liste se consulte pour se rassurer, on ne veut pas y
        croiser le bouton qui écrase la base.
      -->
      {#if $isAdmin}
        <RestoreCard
          archives={backups?.backups ?? []}
          onReload={loadBackups}
          {onExpired}
        />
      {/if}

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

      <!--
        L'inventaire vit en SQLite, pas dans le bucket : la rétention Influx ne
        le voit pas, et il grossit de son côté — une ligne par machine et une
        par port ouvert, à chaque scan. D'où un réglage distinct, sous la même
        barre rouge, avec la même confirmation.
      -->
      <section class="card lp-card danger-frame" class:risky={inventoryShrink}>
        <h2 class="lp-title">{$_('settings.inventory_title')}</h2>
        <p class="sub">{$_('settings.inventory_lead')}</p>

        <label class="lp-field">
          {$_('settings.inventory_label')}
          <span class="unit-row">
            <input
              class="lp-input num"
              type="number"
              min="0"
              step="1"
              bind:value={inventoryDays}
              disabled={!$isAdmin}
            />
            <span class="unit">{$_('settings.retention_unit')}</span>
          </span>
          <span class="hint">{$_('settings.inventory_hint')}</span>
          <span class="cur">
            {$_('settings.retention_current', {
              values: { label: inventoryLabel(saved.inventory_days) },
            })}
          </span>
        </label>

        {#if inventoryShrink}
          <div class="warn" role="status">
            <strong>{$_('settings.shrink_title')}</strong>
            <p>
              {$_('settings.inventory_shrink_from_to', {
                values: {
                  from: inventoryLabel(saved.inventory_days),
                  to: inventoryLabel(inventoryNum),
                },
              })}
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

  <!--
    Une seule confirmation pour les deux réductions destructrices : le hub
    n'expose qu'un `PUT` et qu'un `confirm_data_loss`, qui vaut pour la requête
    entière. Deux boîtes à la suite laisseraient croire à deux envois, et faire
    disparaître la seconde derrière la première laisserait passer une
    suppression non lue. Chaque perte a donc son paragraphe, dans la même
    boîte, et la case les couvre toutes.
  -->
  <Modal
    open={shrinkOpen}
    title={bothLosses
      ? $_('settings.loss_title')
      : isShrink
        ? $_('settings.shrink_title')
        : $_('backup.keep_shrink_title')}
    onclose={() => (shrinkOpen = false)}
  >
    {#if isShrink}
      {#if bothLosses}<strong class="loss-head">{$_('settings.shrink_title')}</strong>{/if}
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
    {/if}

    {#if keepShrink}
      {#if bothLosses}<strong class="loss-head">{$_('backup.keep_shrink_title')}</strong>{/if}
      <p>
        {$_('backup.keep_shrink_from_to', {
          values: { from: keepLabel(saved.backup_keep_last), to: keepLabel(keepNum) },
        })}
      </p>
      <p class="cutoff">{$_('backup.keep_shrink_cutoff')}</p>
    {/if}

    <label class="ack">
      <input type="checkbox" bind:checked={shrinkAck} />
      <span>
        {bothLosses
          ? $_('settings.loss_ack')
          : isShrink
            ? $_('settings.shrink_ack', { values: { date: cutoffDate } })
            : $_('backup.keep_shrink_ack', { values: { to: keepLabel(keepNum) } })}
      </span>
    </label>

    {#snippet footer()}
      <button class="lp-btn" onclick={() => (shrinkOpen = false)}>
        {isShrink && !bothLosses
          ? $_('settings.shrink_keep', { values: { from: retentionLabel(saved.retention_days) } })
          : $_('common.cancel')}
      </button>
      <button class="lp-btn danger" onclick={() => commit(true)} disabled={!shrinkAck || busy}>
        {busy
          ? $_('settings.saving_all')
          : bothLosses
            ? $_('settings.loss_confirm')
            : isShrink
              ? $_('settings.shrink_confirm')
              : $_('backup.keep_shrink_confirm')}
      </button>
    {/snippet}
  </Modal>
{/if}

<style>
  /* ── Mon compte ─────────────────────────────────────────────────────── */
  .block {
    padding: 16px 18px;
  }
  .idlist {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 28px;
    margin: 10px 0 0;
  }
  .idlist dt {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--ep-text-dim);
  }
  .idlist dd {
    margin: 2px 0 0;
    font-size: 14px;
  }
  .pwrow {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    margin-top: 4px;
  }
  .pwerr {
    font-size: 12px;
    color: var(--ep-danger);
  }
  .pwok {
    font-size: 12px;
    color: var(--ep-success);
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
  /* « Enregistré mais pas appliqué » n'est ni une erreur ni un succès : c'est
     une action qui reste à faire. La couleur d'accent le dit sans crier. */
  .pk-list {
    display: flex;
    flex-direction: column;
    margin: 4px 0 10px;
  }
  .pk-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 10px;
    padding: 7px 0;
    border-top: 1px solid var(--ep-border);
    font-size: 12.5px;
  }
  .pk-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pk-when {
    font-size: 11px;
    color: var(--ep-text-secondary);
  }
  /* L'avertissement remplace le bouton : il doit se lire comme une raison,
     pas comme une note de bas de page. */
  .warn-inline {
    margin: 6px 0 0;
    font-size: 12px;
    line-height: 1.5;
    color: var(--ep-accent, #b8860b);
  }
  .qr {
    display: flex;
    justify-content: center;
    margin: 10px 0;
  }
  /* Le SVG arrive avec ses dimensions propres : on les borne, sinon un QR de
     600 px pousse la carte hors de l'écran sur téléphone. */
  .qr :global(svg) {
    width: 100%;
    max-width: 220px;
    height: auto;
    background: #fff;
    padding: 8px;
    border-radius: 6px;
  }
  .secret {
    margin: 4px 0 10px;
    font-size: 13px;
    letter-spacing: 0.12em;
    word-break: break-all;
    user-select: all;
  }
  .code {
    font-family: var(--ep-mono, ui-monospace, monospace);
    letter-spacing: 0.35em;
    text-align: center;
  }
  .pending {
    color: var(--ep-accent, #b8860b);
    font-weight: 600;
  }
  /* L'interrupteur TLS : même forme que les autres cases de cet écran. */
  .switch {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    cursor: pointer;
  }
  .tls-list {
    margin: 6px 0 0;
    padding-left: 18px;
    display: flex;
    flex-direction: column;
    gap: 4px;
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
  /* Titre de bloc dans la boîte, présent uniquement quand elle porte deux
     pertes : avec une seule, le titre du dialogue le dit déjà et le répéter
     ferait lire deux fois la même chose. */
  .loss-head {
    font-size: 12.5px;
    color: var(--ep-text-primary);
    margin-top: 4px;
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
