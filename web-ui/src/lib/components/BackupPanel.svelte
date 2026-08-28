<script lang="ts">
  import { _, locale } from 'svelte-i18n';
  import { api, ApiError, type BackupCreated, type BackupList } from '$lib/api';
  import { humanBytes } from '$lib/time';

  interface Props {
    /** Liste chargée par l'écran parent : une seule lecture pour tout l'onglet. */
    list: BackupList | null;
    listError: string;
    /** Relit la liste après une sauvegarde — la rétention a pu en retirer. */
    onReload: () => void;
    onExpired: () => void;
    /** Réglages du hub, tenus par le parent : ils partent avec « Enregistrer ». */
    enabled: boolean;
    intervalHours: string;
    keepLast: string;
  }

  let {
    list,
    listError,
    onReload,
    onExpired,
    enabled = $bindable(),
    intervalHours = $bindable(),
    keepLast = $bindable(),
  }: Props = $props();

  const lang = $derived($locale ?? 'en');

  // Date ET heure : plusieurs archives peuvent tomber le même jour dès qu'on
  // en déclenche une à la main avant une opération risquée, et « le 28 août »
  // ne dit alors pas laquelle est la bonne. Secondes omises : elles ne servent
  // à distinguer que le nom de fichier, qui reste en clair dans l'infobulle.
  const stamp = $derived(
    new Intl.DateTimeFormat(lang, { dateStyle: 'medium', timeStyle: 'short' }),
  );

  let busy = $state(false);
  let created = $state<BackupCreated | null>(null);
  let createError = $state('');

  async function backupNow() {
    busy = true;
    createError = '';
    created = null;
    try {
      created = await api.createBackup();
      onReload();
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      createError = e instanceof ApiError ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

<!--
  Deux cartes et pas une : « est-ce que ça sauvegarde tout seul ? » et « qu'y
  a-t-il dans le coffre ? » sont deux questions, posées à deux moments. La
  première se règle une fois puis s'oublie ; la seconde se vérifie après un
  incident. Fondues ensemble, le réglage se retrouvait sur le chemin de la
  vérification, à chaque fois.
-->
<section class="card lp-card">
  <h2 class="lp-title">{$_('backup.auto_title')}</h2>

  <label class="switch">
    <input type="checkbox" bind:checked={enabled} />
    <span>{$_('backup.auto_label')}</span>
  </label>

  <!-- Cadence et conservation restent lisibles quand l'automatique est coupé,
       mais désactivées : elles ne règlent plus rien, et un champ modifiable
       qui n'a aucun effet est un piège. -->
  <div class="pair">
    <label class="lp-field">
      {$_('backup.interval_label')}
      <span class="unit-row">
        <input
          class="lp-input num"
          type="number"
          min="1"
          step="1"
          bind:value={intervalHours}
          disabled={!enabled}
        />
        <span class="unit">{$_('backup.interval_unit')}</span>
      </span>
    </label>

    <label class="lp-field">
      {$_('backup.keep_label')}
      <span class="unit-row">
        <input class="lp-input num" type="number" min="0" step="1" bind:value={keepLast} />
        <span class="unit">{$_('backup.keep_unit')}</span>
      </span>
      <span class="hint">{$_('backup.keep_hint')}</span>
    </label>
  </div>
</section>

<section class="card lp-card">
  <div class="head">
    <h2 class="lp-title">{$_('backup.list_title')}</h2>
    <button class="lp-btn accent" onclick={backupNow} disabled={busy}>
      {busy ? $_('backup.now_busy') : $_('backup.now')}
    </button>
  </div>

  {#if createError}<p class="err" role="alert">{createError}</p>{/if}

  {#if created}
    <!-- Une archive sans les mesures reste une archive : elle porte la base,
         les comptes et les secrets. Mais elle ne se lit pas comme un succès
         plein, sinon on la croit complète le jour où on en a besoin. -->
    {#if created.influx_included}
      <p class="ok" role="status">
        {$_('backup.created', {
          values: { file: created.file, size: humanBytes(created.bytes, lang) },
        })}
      </p>
    {:else}
      <div class="warn" role="status">
        <strong>{$_('backup.created_partial_title')}</strong>
        <p>
          {$_('backup.created_partial_body', {
            values: { error: created.influx_error ?? '—' },
          })}
        </p>
      </div>
    {/if}
    {#if created.pruned.length > 0}
      <p class="muted">{$_('backup.pruned', { values: { n: created.pruned.length } })}</p>
    {/if}
  {/if}

  {#if listError}
    <p class="err" role="alert">{$_('backup.load_error')} {listError}</p>
  {:else if list}
    {#if list.backups.length === 0}
      <!-- Deux vides, deux causes : le hub vient d'être posé, ou quelqu'un a
           coupé l'automatique. Un seul message pour les deux laisserait
           attendre une sauvegarde qui ne viendra jamais. -->
      <p class="muted">
        {list.enabled ? $_('backup.empty_enabled') : $_('backup.empty_disabled')}
      </p>
    {:else}
      <div class="scroll">
        <table class="rows">
          <thead>
            <tr>
              <th>{$_('backup.col_date')}</th>
              <th class="right">{$_('backup.col_size')}</th>
              <th class="right">{$_('backup.col_version')}</th>
            </tr>
          </thead>
          <tbody>
            {#each list.backups as a (a.file)}
              <tr>
                <td title={a.file}>{stamp.format(new Date(a.created_at_unix * 1000))}</td>
                <td class="right lp-mono">{humanBytes(a.bytes, lang)}</td>
                <td class="right lp-mono">{a.hub_version}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}

    <p class="dir lp-mono">{$_('backup.dir', { values: { dir: list.dir } })}</p>
  {/if}

  <!-- ⚠️ Une ligne, pas un paragraphe, mais elle ne s'enlève pas : le dossier
       d'archives porte la clé qui déchiffre les identifiants SMTP et les URL
       de webhook. Qui le copie sur un partage ouvert copie les secrets. -->
  <p class="secret">{$_('backup.secret')}</p>
</section>

<style>
  .card {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .head {
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
  .hint {
    font-size: 11px;
    color: var(--ep-text-muted);
    line-height: 1.55;
    max-width: 74ch;
  }

  /* Le tableau défile DANS sa carte plutôt que de pousser la page : sous
     390 px, trois colonnes tiennent, mais un nom de dossier ou une version
     inattendue ne doivent jamais faire glisser tout l'écran de côté. */
  .scroll {
    overflow-x: auto;
  }
  .rows {
    width: 100%;
    border-collapse: collapse;
    font-size: 11.5px;
  }
  .rows th {
    text-align: left;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--ep-text-dim);
    padding: 5px 8px;
    border-bottom: 1px solid var(--ep-border);
    font-weight: 500;
    white-space: nowrap;
  }
  .rows td {
    padding: 7px 8px;
    border-bottom: 1px solid var(--ep-border);
    color: var(--ep-text-secondary);
    white-space: nowrap;
  }
  .rows tr:last-child td {
    border-bottom: none;
  }
  /* La taille et la version se comparent d'une ligne à l'autre : alignées à
     droite, les chiffres tombent sur la même colonne d'unités et une archive
     anormalement petite — celle du soir où Influx ne répondait pas — se voit
     sans la lire. */
  .rows .right {
    text-align: right;
  }

  .dir {
    font-size: 10.5px;
    color: var(--ep-text-muted);
    margin: 0;
    overflow-wrap: anywhere;
  }
  .secret {
    font-size: 11px;
    line-height: 1.55;
    color: var(--ep-text-secondary);
    border-left: 2px solid var(--ep-warning);
    padding-left: 10px;
    margin: 0;
    max-width: 74ch;
  }
  .muted {
    color: var(--ep-text-muted);
    font-size: 12px;
    line-height: 1.6;
    margin: 0;
    max-width: 74ch;
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
    overflow-wrap: anywhere;
  }
  .warn {
    border: 1px solid color-mix(in srgb, var(--ep-warning) 45%, var(--ep-border));
    border-left-width: 3px;
    border-radius: var(--ep-radius-md);
    background: color-mix(in srgb, var(--ep-warning) 8%, transparent);
    padding: 11px 13px;
  }
  .warn strong {
    font-size: 12.5px;
    color: var(--ep-text-primary);
    display: block;
    margin-bottom: 4px;
  }
  .warn p {
    font-size: 11.5px;
    color: var(--ep-text-secondary);
    line-height: 1.6;
    margin: 0;
    max-width: 78ch;
    overflow-wrap: anywhere;
  }
</style>
