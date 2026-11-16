<script lang="ts">
  import { onMount } from 'svelte';
  import { _, locale } from 'svelte-i18n';
  import { api, ApiError, AUDIT_ACTIONS, type AuditEntry } from '$lib/api';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import { absoluteTime, logTime, relativeTime, now } from '$lib/time';
  import { auditActor as actorOf } from '$lib/audit-actors';
  import { auditDetail, auditTarget } from '$lib/audit-detail';
  import { fleet } from '$lib/fleet';
  import { devicesApi } from '$lib/devices';

  const { onExpired } = $props<{ onExpired: () => void }>();
  const lang = $derived($locale ?? 'en');

  /** Le hub plafonne à 500 ; 100 est son défaut et tient sur un écran de suite. */
  const PAGE = 100;

  let entries = $state<AuditEntry[]>([]);
  let nextBeforeId = $state<number | null>(null);
  let loading = $state(true);
  let loadingMore = $state(false);
  let loadError = $state('');
  let denied = $state('');

  // Brouillon des filtres : ils ne s'appliquent qu'à la validation, sinon
  // chaque frappe partirait au hub et la liste sauterait sous les doigts.
  let actorDraft = $state('');
  let actionDraft = $state('');
  let actor = $state('');
  let action = $state('');
  const filtered = $derived(actor !== '' || action !== '');

  /**
   * Un 401 renvoie à la connexion, un 403 non : le contrat § 11 prévoit
   * précisément ce cas — se reconnecter avec les mêmes identifiants donnerait
   * le même refus.
   */
  function handle(e: unknown): string {
    if (e instanceof ApiError && e.isExpired) {
      onExpired();
      return '';
    }
    return e instanceof ApiError ? e.message : String(e);
  }

  async function load() {
    loading = true;
    loadError = '';
    denied = '';
    try {
      const page = await api.audit({ limit: PAGE, actor, action });
      entries = page?.entries ?? [];
      nextBeforeId = page?.next_before_id ?? null;
    } catch (e) {
      const msg = handle(e);
      if (e instanceof ApiError && e.isForbidden) denied = msg;
      else loadError = msg;
      entries = [];
      nextBeforeId = null;
    } finally {
      loading = false;
    }
  }

  /**
   * Pagination à rebours, donc on ajoute à la suite au lieu d'afficher
   * « précédent / suivant ». Le curseur `before_id` ne sait aller que dans un
   * sens : proposer un retour arrière obligerait à rejouer toutes les pages, et
   * un `OFFSET` rejouerait des lignes déjà vues dès qu'une ligne s'ajoute
   * pendant la lecture.
   */
  async function loadMore() {
    if (nextBeforeId == null || loadingMore) return;
    loadingMore = true;
    loadError = '';
    try {
      const page = await api.audit({ limit: PAGE, before_id: nextBeforeId, actor, action });
      entries = [...entries, ...(page?.entries ?? [])];
      nextBeforeId = page?.next_before_id ?? null;
    } catch (e) {
      loadError = handle(e);
    } finally {
      loadingMore = false;
    }
  }

  function apply(e?: Event) {
    e?.preventDefault();
    actor = actorDraft.trim();
    action = actionDraft;
    void load();
  }

  function reset() {
    actorDraft = '';
    actionDraft = '';
    actor = '';
    action = '';
    void load();
  }

  /**
   * Les noms que le détail peut citer : sites, sondes, appareils de ce compte.
   * Lus à part, au mieux : sans eux, le détail retire l'identifiant au lieu
   * de l'afficher.
   */
  let deviceNames = $state<Record<string, string>>({});
  const names = $derived.by(() => {
    const out: Record<string, string> = { ...deviceNames };
    for (const s of $fleet.sites) out[s.site_id] = s.name;
    for (const p of $fleet.probes) out[p.probe_id] = p.name;
    return out;
  });
  const opts = $derived({ lang, names, at: $now, t: (k: string, v?: Record<string, unknown>) => $_(k, v as never) });
  const detailOf = (raw: string | null | undefined) => auditDetail(raw, opts);

  /** « Échecs » trie ce qui est chargé : le hub ne filtre pas sur le résultat. */
  let onlyFailures = $state(false);
  const visible = $derived(onlyFailures ? entries.filter((e) => e.outcome === 'failure') : entries);

  onMount(() => {
    void load();
    if ($fleet.sites.length === 0) void fleet.load(onExpired, { quiet: true });
    devicesApi
      .myDevices()
      .then((list) => {
        const out: Record<string, string> = {};
        for (const d of (list as { device_id: string; name: string }[] | null) ?? []) out[d.device_id] = d.name;
        deviceNames = out;
      })
      .catch(() => {});
  });

  // Les actions sont groupées par préfixe (`auth.`, `user.`, `probe.`…) : la
  // liste en compte trente-deux, et un `optgroup` par famille évite de la
  // parcourir en entier pour trouver « probe.revoke ».
  const grouped = $derived.by(() => {
    const out = new Map<string, string[]>();
    for (const a of AUDIT_ACTIONS) {
      const family = a.split('.')[0];
      const list = out.get(family) ?? [];
      list.push(a);
      out.set(family, list);
    }
    return [...out.entries()];
  });

  /**
   * Libellé lisible d'une action.
   *
   * Le code (`auth.login`) reste la vérité : il sert au filtre, il est le même
   * dans les journaux du conteneur, et une action inconnue de la traduction —
   * une version plus récente du hub, par exemple — doit rester lisible plutôt
   * que de disparaître. Il est donc conservé en titre de l'élément.
   */
  function actionLabel(code: string): string {
    const key = `audit.action.${code.replace(/\./g, '_')}`;
    const label = $_(key);
    return label === key ? code : label;
  }

  function familyLabel(family: string): string {
    const key = `audit.family.${family}`;
    const label = $_(key);
    return label === key ? family : label;
  }

  const failures = $derived(entries.filter((e) => e.outcome === 'failure').length);
</script>

<header class="lp-head">
  <div>
    <h1>{$_('audit.title')}</h1>
    {#if !loading && !denied && !loadError}
      <p class="lp-sub">
        {$_('audit.loaded', { values: { n: entries.length } })}{#if failures > 0},
          <span class="fcount">{$_('audit.failures', { values: { n: failures } })}</span>{/if}
      </p>
    {/if}
  </div>
  {#if !denied}
    <div class="lp-acts">
      <button class="lp-btn" onclick={load} disabled={loading}>{$_('audit.refresh')}</button>
    </div>
  {/if}
</header>

{#if denied}
  <StateBlock tone="error" title={$_('common.forbidden_title')} body={denied} />
{:else}
  <form class="filters" onsubmit={apply}>
    <!-- Liste fermée plutôt que saisie libre : le vocabulaire est fixé par le
         contrat, et une faute de frappe rendrait une liste vide sans dire
         qu'elle vient de la frappe. -->
    <select
      aria-label={$_('audit.filter_action')}
      value={actionDraft}
      onchange={(ev) => {
        // Lecture directe de l'élément plutôt que `bind:value` : on ne veut
        // pas dépendre de l'ordre dans lequel Svelte pose le liage et ce
        // gestionnaire, sinon le filtre part avec la valeur précédente.
        actionDraft = ev.currentTarget.value;
        apply();
      }}
    >
      <option value="">{$_('audit.filter_all_actions')}</option>
      {#each grouped as [family, list] (family)}
        <optgroup label={familyLabel(family)}>
          {#each list as a (a)}
            <option value={a}>{actionLabel(a)}</option>
          {/each}
        </optgroup>
      {/each}
    </select>
    <input
      class="lp-input actor-in"
      aria-label={$_('audit.filter_actor')}
      bind:value={actorDraft}
      spellcheck="false"
      autocomplete="off"
      placeholder={$_('audit.filter_actor_placeholder')}
    />
    <button class="lp-btn" type="submit">{$_('audit.apply')}</button>
    {#if filtered}
      <button class="lp-btn ghost" type="button" onclick={reset}>{$_('audit.clear')}</button>
    {/if}
    <div class="lp-seg" role="group" aria-label={$_('audit.col_outcome')}>
      <button type="button" aria-pressed={!onlyFailures} onclick={() => (onlyFailures = false)}>
        {$_('audit.show_all')}
      </button>
      <button type="button" aria-pressed={onlyFailures} onclick={() => (onlyFailures = true)}>
        {$_('audit.show_failures')}
      </button>
    </div>
  </form>

  {#if loading}
    <StateBlock tone="loading" title={$_('common.loading')} />
  {:else if loadError && entries.length === 0}
    <StateBlock tone="error" title={$_('audit.error_title')} body={loadError}>
      <button class="lp-btn primary" onclick={load}>{$_('common.retry')}</button>
    </StateBlock>
  {:else if visible.length === 0}
    <StateBlock
      tone="empty"
      title={filtered || onlyFailures ? $_('audit.empty_filtered_title') : $_('audit.empty_title')}
      body={filtered ? $_('audit.empty_filtered_body') : undefined}
    >
      {#if filtered}
        <button class="lp-btn primary" onclick={reset}>{$_('audit.clear')}</button>
      {/if}
    </StateBlock>
  {:else}
    <!--
      Les échecs sont la seule chose colorée de la page : un succès est rendu
      sans marque, un échec porte un liseré rouge et le seul mot coloré de sa
      ligne. Sur cent lignes, l'anomalie se voit avant d'être lue.
    -->
    <div class="lp-tbl-wrap">
      <table class="lp-t">
        <thead>
          <tr>
            <th>{$_('audit.col_when')}</th>
            <th>{$_('audit.col_actor')}</th>
            <th>{$_('audit.col_action')}</th>
            <th>{$_('audit.col_target')}</th>
            <th>{$_('audit.col_detail')}</th>
            <th>{$_('audit.col_outcome')}</th>
          </tr>
        </thead>
        <tbody>
          {#each visible as e (e.id)}
            {@const tg = auditTarget(e, opts)}
            <tr class:failure={e.outcome === 'failure'}>
              <!-- ⚠️ Horodatage réel, pas « il y a 2 h » : un journal d'audit se
                   recoupe avec d'autres traces. Le relatif reste en infobulle.
                   `Math.max` : une ligne d'audit est toujours dans le passé. -->
              <td
                class="when"
                title={`${absoluteTime(e.at, lang)} — ${relativeTime(e.at, lang, Math.max($now, e.at * 1000))}`}
              >
                {logTime(e.at, lang)}
              </td>
              <!-- 🔴 Trois réponses, pas deux : un compte, le hub lui-même
                   (sauvegarde planifiée), ou un inconnu (connexion sur un compte
                   qui n'existe pas). Le hub ne recopie jamais la saisie ici. -->
              <td>
                {#if actorOf(e).kind === 'account'}
                  {e.actor}
                {:else if actorOf(e).kind === 'system'}
                  <span class="anon">{$_('audit.system')}</span>
                {:else}
                  <span class="anon">{$_('audit.anonymous')}</span>
                {/if}
              </td>
              <td title={e.action}>{actionLabel(e.action)}</td>
              <!--
                Le nom d'AUJOURD'HUI, résolu à la lecture. 🔴 L'identifiant
                reste le fait : il est en infobulle quand un nom existe, et
                raccourci quand la chose n'existe plus — jamais « inconnu »
                déguisé en nom.
              -->
              <td class="target" title={e.target ?? ''}>
                {#if tg.mono}<span class="lp-mono">{tg.text}</span>{:else}{tg.text}{/if}
              </td>
              <td class="detail">{detailOf(e.detail)}</td>
              <td class="outcome">
                {#if e.outcome === 'failure'}
                  <span class="bad">{$_('audit.failure')}</span>
                {:else}
                  <span class="good">{$_('audit.success')}</span>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    {#if loadError}<p class="err" role="alert">{loadError}</p>{/if}

    {#if nextBeforeId != null}
      <div class="more">
        <button class="lp-btn" onclick={loadMore} disabled={loadingMore}>
          {loadingMore ? $_('audit.loading_more') : $_('audit.load_more')}
        </button>
      </div>
    {:else}
      <p class="end">{$_('audit.end')}</p>
    {/if}
  {/if}
{/if}

<style>
  .fcount {
    color: var(--st-bad);
    font-weight: 500;
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-bottom: 16px;
  }
  .filters select {
    max-width: 280px;
  }
  .actor-in {
    width: 200px;
  }
  .filters .lp-seg {
    margin-left: auto;
  }
  .when {
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
    color: var(--color-ink-2);
  }
  .anon {
    color: var(--color-muted);
  }
  .target {
    max-width: 240px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .target .lp-mono {
    font-size: 0.8125rem;
    color: var(--color-ink-2);
  }
  .detail {
    color: var(--color-ink-2);
    white-space: normal;
    min-width: 200px;
  }
  .good {
    color: var(--color-muted);
  }
  .bad {
    color: var(--st-bad);
    font-weight: 500;
  }
  tr.failure td:first-child {
    box-shadow: inset 2px 0 0 var(--st-bad);
  }
  tr.failure td {
    background: var(--st-bad-wash);
  }
  .more {
    display: flex;
    justify-content: center;
    margin-top: 16px;
  }
  .end {
    text-align: center;
    color: var(--color-muted);
    font-size: 0.8125rem;
    margin: 16px 0 0;
  }
  .err {
    color: var(--st-bad);
    font-size: 0.8125rem;
    margin: 12px 0 0;
  }
</style>
