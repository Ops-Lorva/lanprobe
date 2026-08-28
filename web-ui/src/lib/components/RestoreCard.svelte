<script lang="ts">
  import { _, locale } from 'svelte-i18n';
  import { api, ApiError, type BackupArchive, type RestoreReport } from '$lib/api';
  import Modal from '$lib/components/Modal.svelte';
  import { humanBytes } from '$lib/time';

  interface Props {
    /** Archives déjà sur le hub, les plus récentes en tête. */
    archives: BackupArchive[];
    /** La restauration a pu produire une archive de plus (le téléversement est gardé). */
    onReload: () => void;
    onExpired: () => void;
  }
  let { archives, onReload, onExpired }: Props = $props();

  const lang = $derived($locale ?? 'en');
  const stamp = $derived(
    new Intl.DateTimeFormat(lang, { dateStyle: 'medium', timeStyle: 'short' }),
  );

  let file = $state<File | null>(null);
  let input = $state<HTMLInputElement | null>(null);
  let picked = $state('');

  // La cible de la restauration : un fichier de la machine, ou une archive du
  // hub. Une seule confirmation pour les deux — c'est le même remplacement.
  type Target = { kind: 'file'; file: File } | { kind: 'archive'; name: string };
  let target = $state<Target | null>(null);
  let ack = $state(false);
  let busy = $state(false);

  let report = $state<RestoreReport | null>(null);
  let reportOpen = $state(false);

  /**
   * Un refus ne se lit pas pareil selon d'où il vient. 400 : le fichier remis
   * n'est pas une archive LanProbe, c'est à l'utilisateur de changer de
   * fichier. 409 : l'archive est valide mais vient d'un hub plus récent, c'est
   * le hub qu'il faut mettre à jour. Le reste : la panne est chez le hub, il
   * n'y a rien à corriger côté fichier. Les confondre envoie chercher un
   * problème là où il n'y en a pas.
   */
  let failure = $state<{ title: string; body: string } | null>(null);

  const selected = $derived(archives.find((a) => a.file === picked) ?? null);

  // `created_at` arrive en RFC 3339 (« 2026-08-28T22:18:52Z ») : lisible par
  // une machine, pas par quelqu'un qui cherche « celle d'avant-hier soir ».
  const reportDate = $derived(report ? stamp.format(new Date(report.created_at)) : '');

  function label(a: BackupArchive) {
    return $_('backup.option', {
      values: {
        date: stamp.format(new Date(a.created_at_unix * 1000)),
        size: humanBytes(a.bytes, lang),
        version: a.hub_version,
      },
    });
  }

  function onPick(e: Event) {
    file = (e.currentTarget as HTMLInputElement).files?.[0] ?? null;
  }

  function ask(t: Target) {
    failure = null;
    ack = false;
    target = t;
  }

  const targetName = $derived(
    target === null ? '' : target.kind === 'file' ? target.file.name : target.name,
  );

  async function run() {
    if (!target) return;
    busy = true;
    failure = null;
    try {
      // `confirm_overwrite` toujours vrai ici : c'est la case cochée juste
      // au-dessus qui l'a autorisé. L'envoyer à faux ferait répondre 409 au
      // hub pour une confirmation qui vient d'être donnée.
      const r =
        target.kind === 'file'
          ? await api.restoreUpload(target.file, true)
          : await api.restoreBackup(target.name, true);
      target = null;
      report = r;
      reportOpen = true;
      // Le téléversement est conservé par le hub sous son nom canonique : la
      // liste en compte donc une de plus qu'avant.
      onReload();
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      const status = e instanceof ApiError ? e.status : -1;
      const body = e instanceof ApiError ? e.message : String(e);
      const title =
        status === 400
          ? $_('backup.err_file_title')
          : status === 409
            ? $_('backup.err_schema_title')
            : $_('backup.err_hub_title');
      target = null;
      failure = { title, body };
    } finally {
      busy = false;
    }
  }

  function clearFile() {
    file = null;
    if (input) input.value = '';
  }
</script>

<section class="card lp-card danger-frame">
  <h2 class="lp-title">{$_('backup.restore_title')}</h2>
  <p class="sub">{$_('backup.restore_lead')}</p>

  <!-- Le téléversement d'abord : c'est le geste de celui qui remonte un hub
       neuf, et son archive est sur SA machine, pas dans le volume d'un hub qui
       n'existe plus. La liste locale ne sert qu'au cas plus rare — revenir en
       arrière sur un hub encore debout. -->
  <div class="row">
    <label class="lp-field grow">
      {$_('backup.file_label')}
      <input
        class="file"
        type="file"
        accept=".zip,application/zip"
        bind:this={input}
        onchange={onPick}
      />
    </label>
    <button
      class="lp-btn danger"
      disabled={!file || busy}
      onclick={() => file && ask({ kind: 'file', file })}
    >
      {$_('backup.restore_file')}
    </button>
  </div>

  <div class="rule" aria-hidden="true"></div>

  <div class="row">
    <label class="lp-field grow">
      {$_('backup.or_existing')}
      {#if archives.length === 0}
        <p class="muted">{$_('backup.no_archive')}</p>
      {:else}
        <select class="lp-input" bind:value={picked}>
          <option value="">{$_('backup.pick')}</option>
          {#each archives as a (a.file)}
            <option value={a.file}>{label(a)}</option>
          {/each}
        </select>
      {/if}
    </label>
    <button
      class="lp-btn danger"
      disabled={!selected || busy}
      onclick={() => selected && ask({ kind: 'archive', name: selected.file })}
    >
      {$_('backup.restore_existing')}
    </button>
  </div>

  {#if failure}
    <div class="warn" role="alert">
      <strong>{failure.title}</strong>
      <p>{failure.body}</p>
    </div>
  {/if}

  <!-- Le compte rendu reste sur la carte après la fermeture du dialogue : le
       chemin de l'état mis de côté est la seule trace de la marche arrière
       possible, et on le cherche plus tard, pas dans la seconde. -->
  {#if report}
    <div class="done" class:partial={!report.influx_restored} role="status">
      <strong>
        {report.influx_restored ? $_('backup.done_title') : $_('backup.done_partial_title')}
      </strong>
      <p class="restart">
        <strong class="rt">{$_('backup.restart_title')}</strong>
        {$_('backup.restart_body')}
      </p>
      <p>{$_('backup.from', { values: { date: reportDate, version: report.hub_version } })}</p>
      {#if report.restored.length > 0}
        <p class="lp-mono small">
          {$_('backup.replaced', { values: { list: report.restored.join(', ') } })}
        </p>
      {/if}
      <p class:ko={!report.influx_restored}>
        {report.influx_restored
          ? $_('backup.influx_ok')
          : $_('backup.influx_ko', { values: { error: report.influx_error ?? '—' } })}
      </p>
      {#if report.moved_aside}
        <p>{$_('backup.aside')}</p>
        <p class="lp-mono small">{report.moved_aside}</p>
      {/if}
    </div>
  {/if}
</section>

<!-- Confirmation : elle nomme ce qui sera remplacé, dit où part l'état
     précédent, et annonce le redémarrage avant le clic plutôt qu'après. -->
<Modal
  open={target !== null}
  wide
  title={$_('backup.confirm_title')}
  onclose={() => (target = null)}
>
  <p class="lp-mono id">{targetName}</p>
  <p>{$_('backup.confirm_replaced')}</p>
  <p class="aside-note">{$_('backup.confirm_aside')}</p>
  <p>{$_('backup.confirm_restart')}</p>

  <label class="ack">
    <input type="checkbox" bind:checked={ack} />
    <span>{$_('backup.confirm_ack')}</span>
  </label>

  {#snippet footer()}
    <button class="lp-btn" onclick={() => (target = null)}>{$_('common.cancel')}</button>
    <button class="lp-btn danger" onclick={run} disabled={!ack || busy}>
      {busy ? $_('backup.restoring') : $_('backup.confirm_go')}
    </button>
  {/snippet}
</Modal>

<!-- Le compte rendu s'ouvre en dialogue et non en simple ligne : « il faut
     redémarrer » est LE message de cet écran, et sans lui on regarde ses
     anciennes données en concluant que la restauration a échoué. Un dialogue
     se ferme à la main, une ligne se rate. -->
<Modal
  open={reportOpen}
  wide
  title={report?.influx_restored ? $_('backup.done_title') : $_('backup.done_partial_title')}
  onclose={() => {
    reportOpen = false;
    clearFile();
  }}
>
  {#if report}
    <div class="warn restart-block">
      <strong>{$_('backup.restart_title')}</strong>
      <p>{$_('backup.restart_body')}</p>
    </div>
    {#if !report.influx_restored}
      <p class="ko">{$_('backup.influx_ko', { values: { error: report.influx_error ?? '—' } })}</p>
    {:else}
      <p>{$_('backup.influx_ok')}</p>
    {/if}
    <p>{$_('backup.from', { values: { date: reportDate, version: report.hub_version } })}</p>
    {#if report.moved_aside}
      <p>{$_('backup.aside')}</p>
      <p class="lp-mono small">{report.moved_aside}</p>
    {/if}
  {/if}
  {#snippet footer()}
    <button
      class="lp-btn primary"
      onclick={() => {
        reportOpen = false;
        clearFile();
      }}
    >
      {$_('backup.done_ack')}
    </button>
  {/snippet}
</Modal>

<style>
  .card {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
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
  /* Le bouton s'aligne en bas du champ qu'il consomme, et repasse sous lui
     quand la largeur ne suffit plus : à 390 px, un champ de fichier et un
     bouton côte à côte ne laissent lisible ni l'un ni l'autre. */
  .row {
    display: flex;
    align-items: flex-end;
    gap: 10px;
    flex-wrap: wrap;
  }
  .grow {
    flex: 1 1 240px;
    min-width: 0;
  }
  .file {
    width: 100%;
    font-family: var(--ep-font-sans);
    font-size: 12px;
    color: var(--ep-text-secondary);
    min-width: 0;
  }
  .file::file-selector-button {
    margin-right: 10px;
    padding: 7px 12px;
    border-radius: var(--ep-radius-md);
    border: 1px solid var(--ep-border-strong);
    background: var(--ep-bg-tertiary);
    color: var(--ep-text-primary);
    font-family: var(--ep-font-sans);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
  }
  /* Le libellé d'une option porte trois informations sur une seule ligne, et
     un <select> natif tronque sans prévenir : à 390 px, un point de moins par
     caractère rend la version du hub visible, et c'est elle qui décide si
     l'archive est restaurable. */
  select.lp-input {
    font-size: 12px;
  }
  .rule {
    height: 1px;
    background: var(--ep-border);
  }
  .muted {
    color: var(--ep-text-muted);
    font-size: 12px;
    margin: 0;
  }
  .small {
    font-size: 10.5px;
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
  }
  .restart-block {
    margin: 0;
  }

  /* Vert pour une restauration entière, ambre dès qu'il manque les mesures.
     Le même cadre pour les deux ferait passer une restauration partielle pour
     un succès — c'est exactement l'erreur qu'on ne peut pas se permettre ici. */
  .done {
    border: 1px solid color-mix(in srgb, var(--ep-success) 45%, var(--ep-border));
    border-left-width: 3px;
    border-radius: var(--ep-radius-md);
    background: color-mix(in srgb, var(--ep-success) 7%, transparent);
    padding: 11px 13px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .done.partial {
    border-color: color-mix(in srgb, var(--ep-warning) 45%, var(--ep-border));
    background: color-mix(in srgb, var(--ep-warning) 8%, transparent);
  }
  .done > strong {
    font-size: 12.5px;
    color: var(--ep-text-primary);
  }
  .done p {
    font-size: 11.5px;
    color: var(--ep-text-secondary);
    line-height: 1.6;
    margin: 0;
    max-width: 78ch;
    overflow-wrap: anywhere;
  }
  .restart {
    border-left: 2px solid var(--ep-danger);
    padding-left: 10px;
  }
  .rt {
    color: var(--ep-danger);
    display: block;
    font-size: 12px;
  }
  .ko {
    color: var(--ep-warning);
  }

  .id {
    font-size: 11.5px;
    color: var(--ep-text-primary);
    overflow-wrap: anywhere;
    margin: 0;
  }
  .aside-note {
    border-left: 2px solid var(--ep-success);
    padding-left: 10px;
    margin: 0;
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
