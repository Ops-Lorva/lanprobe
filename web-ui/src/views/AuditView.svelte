<script lang="ts">
  import { onMount } from 'svelte';
  import { _, locale } from 'svelte-i18n';
  import { api, ApiError, AUDIT_ACTIONS, type AuditEntry } from '$lib/api';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import { absoluteTime, relativeTime, now } from '$lib/time';

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

  onMount(load);

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

<header class="head">
  <div class="titles">
    <h1>{$_('audit.title')}</h1>
    {#if !loading && !denied && !loadError}
      <p class="count">
        {$_('audit.loaded', { values: { n: entries.length } })}
        {#if failures > 0}
          <span class="fcount">{$_('audit.failures', { values: { n: failures } })}</span>
        {/if}
      </p>
    {/if}
  </div>
  {#if !denied}
    <button class="lp-btn" onclick={load} disabled={loading}>{$_('audit.refresh')}</button>
  {/if}
</header>

{#if denied}
  <StateBlock tone="error" title={$_('common.forbidden_title')} body={denied} />
{:else}
  <form class="filters" onsubmit={apply}>
    <label class="lp-field grow">
      {$_('audit.filter_actor')}
      <input
        class="lp-input"
        bind:value={actorDraft}
        spellcheck="false"
        autocomplete="off"
        placeholder={$_('audit.filter_actor_placeholder')}
      />
    </label>
    <label class="lp-field grow">
      {$_('audit.filter_action')}
      <!-- Liste fermée plutôt que saisie libre : le vocabulaire est fixé par le
           contrat, et une faute de frappe rendrait une liste vide sans dire
           qu'elle vient de la frappe. -->
      <select
        class="lp-input"
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
    </label>
    <div class="fbtns">
      <button class="lp-btn primary" type="submit">{$_('audit.apply')}</button>
      {#if filtered}
        <button class="lp-btn ghost" type="button" onclick={reset}>{$_('audit.clear')}</button>
      {/if}
    </div>
  </form>

  {#if loading}
    <StateBlock tone="loading" title={$_('common.loading')} />
  {:else if loadError && entries.length === 0}
    <StateBlock tone="error" title={$_('audit.error_title')} body={loadError}>
      <button class="lp-btn primary" onclick={load}>{$_('common.retry')}</button>
    </StateBlock>
  {:else if entries.length === 0}
    <StateBlock
      tone="empty"
      title={filtered ? $_('audit.empty_filtered_title') : $_('audit.empty_title')}
      body={filtered ? $_('audit.empty_filtered_body') : $_('audit.empty_body')}
    >
      {#if filtered}
        <button class="lp-btn primary" onclick={reset}>{$_('audit.clear')}</button>
      {/if}
    </StateBlock>
  {:else}
    <div class="list lp-card">
      <div class="row header" aria-hidden="true">
        <span>{$_('audit.col_when')}</span>
        <span>{$_('audit.col_actor')}</span>
        <span>{$_('audit.col_action')}</span>
        <span>{$_('audit.col_target')}</span>
        <span>{$_('audit.col_outcome')}</span>
      </div>

      <!--
        Les échecs sont la seule chose colorée de la page.

        Le réflexe serait de peindre les succès en vert et les échecs en rouge :
        le journal devient alors un mur de couleur où plus rien ne ressort, et
        une série de `auth.login` en échec — ce qu'on vient chercher ici — se
        noie dans le vert. Un succès est donc rendu en gris, sans marque ; un
        échec porte un liseré rouge, un fond teinté et le seul mot coloré de sa
        ligne. Sur un écran de cent lignes, l'anomalie se voit avant d'être lue.
      -->
      {#each entries as e (e.id)}
        <div class="row" class:failure={e.outcome === 'failure'}>
          <!-- `Math.max` : une ligne d'audit est toujours dans le passé. Une
               seconde d'écart entre l'horloge du hub et celle du poste
               affichait « dans 2 s » sur l'entrée qu'on vient de provoquer. -->
          <span class="when lp-mono" title={absoluteTime(e.at, lang)}>
            {relativeTime(e.at, lang, Math.max($now, e.at * 1000))}
          </span>

          <span class="actor">
            {#if e.actor}
              {e.actor}
            {:else}
              <!-- `actor` nul = tentative anonyme. Le hub ne recopie jamais la
                   saisie : un jour, ce champ recevrait un mot de passe tapé
                   dans la mauvaise case. -->
              <em class="anon">{$_('audit.anonymous')}</em>
            {/if}
          </span>

          <span class="action" title={e.action}>{actionLabel(e.action)}</span>

          <span class="target lp-mono">{e.target ?? '—'}</span>

          <span class="outcome">
            {#if e.outcome === 'failure'}
              <span class="dot" aria-hidden="true"></span>
              <span class="bad">{$_('audit.failure')}</span>
            {:else}
              <span class="good">{$_('audit.success')}</span>
            {/if}
          </span>

          {#if e.detail}
            <span class="detail">{e.detail}</span>
          {/if}
        </div>
      {/each}
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
  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
    margin-bottom: 14px;
  }
  h1 {
    font-size: 20px;
    font-weight: 800;
    letter-spacing: -0.02em;
    margin: 0;
  }
  .count {
    font-size: 12px;
    color: var(--ep-text-secondary);
    margin: 3px 0 0;
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  /* Le compte d'échecs est la seule information colorée de l'en-tête : c'est le
     chiffre qui décide si on lit la page ou si on la referme. */
  .fcount {
    color: var(--ep-danger);
    font-weight: 600;
  }

  .filters {
    display: flex;
    align-items: flex-end;
    gap: 10px;
    flex-wrap: wrap;
    margin-bottom: 12px;
  }
  /* Les filtres ne s'étalent pas : un nom de compte et une liste d'actions ont
     une largeur de confort, et un champ de 600 px se lit comme une barre de
     recherche globale — ce qu'il n'est pas. */
  .grow {
    flex: 1 1 200px;
    max-width: 300px;
    min-width: 0;
  }
  .fbtns {
    display: flex;
    gap: 6px;
  }
  select.lp-input {
    font-family: var(--ep-font-sans);
  }

  .list {
    overflow: hidden;
  }
  .row {
    display: grid;
    grid-template-columns: 92px 130px 170px minmax(0, 1fr) 84px;
    align-items: baseline;
    gap: 4px 10px;
    padding: 8px 14px 8px 11px;
    border-left: 3px solid transparent;
    border-bottom: 1px solid var(--ep-border);
    font-size: 12px;
  }
  .row:last-child {
    border-bottom: none;
  }
  .row.header {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--ep-text-dim);
    background: var(--ep-bg-tertiary);
    padding: 7px 14px 7px 11px;
  }
  .row.failure {
    border-left-color: var(--ep-danger);
    background: color-mix(in srgb, var(--ep-danger) 6%, transparent);
  }

  .when {
    font-size: 11px;
    color: var(--ep-text-muted);
    white-space: nowrap;
  }
  .actor {
    color: var(--ep-text-primary);
    font-weight: 500;
    overflow-wrap: anywhere;
  }
  .anon {
    color: var(--ep-text-muted);
    font-weight: 400;
  }
  .action {
    font-size: 11.5px;
    color: var(--ep-text-secondary);
    overflow-wrap: anywhere;
  }
  .target {
    font-size: 11px;
    color: var(--ep-text-muted);
    overflow-wrap: anywhere;
  }
  .outcome {
    display: flex;
    align-items: center;
    gap: 5px;
    justify-content: flex-end;
    font-size: 11px;
    white-space: nowrap;
  }
  .good {
    color: var(--ep-text-dim);
  }
  .bad {
    color: var(--ep-danger);
    font-weight: 700;
  }
  /* La pastille double le mot : la couleur seule ne se lit pas en vision
     déficiente, et c'est la colonne qu'on vient chercher. */
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: var(--ep-danger);
    flex-shrink: 0;
  }
  .detail {
    grid-column: 3 / -1;
    font-size: 11px;
    line-height: 1.5;
    color: var(--ep-text-muted);
    overflow-wrap: anywhere;
  }

  .more {
    display: flex;
    justify-content: center;
    margin-top: 12px;
  }
  .end {
    text-align: center;
    font-size: 11px;
    color: var(--ep-text-dim);
    margin: 12px 0 0;
  }
  .err {
    font-size: 12px;
    color: var(--ep-danger);
    margin: 10px 0 0;
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
  }

  /* La cible tombe avant le reste : elle est souvent un identifiant de sonde
     illisible de toute façon, et le détail la reprend en clair quand elle
     compte. À 390 px il reste l'essentiel — quand, qui, quoi, et le verdict. */
  @media (max-width: 900px) {
    .row {
      grid-template-columns: 92px 120px minmax(0, 1fr) 78px;
    }
    .target,
    .row.header span:nth-child(4) {
      display: none;
    }
    .detail {
      grid-column: 1 / -1;
    }
  }
  @media (max-width: 560px) {
    .row {
      grid-template-columns: minmax(0, 1fr) auto;
      row-gap: 3px;
    }
    .row.header {
      display: none;
    }
    .when {
      grid-column: 1;
    }
    .outcome {
      grid-row: 1;
      grid-column: 2;
    }
    .actor,
    .action {
      grid-column: 1 / -1;
    }
    .action {
      font-size: 12px;
      color: var(--ep-text-primary);
    }
  }
</style>
