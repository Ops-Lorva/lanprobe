<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import { _, locale } from 'svelte-i18n';
  import { fleet } from '$lib/fleet';
  import { flash } from '$lib/flash';
  import { go } from '$lib/router';
  import { now, relativeTime, absoluteTime, dateOnly, tooltipTime, logTime } from '$lib/time';
  import {
    api,
    ApiError,
    type ProbeCommand,
    type Scan,
    type SpeedtestRow,
  } from '$lib/api';
  import {
    MEASUREMENT,
    RANGES,
    DEFAULT_RANGE,
    MetricsShapeError,
    normalizeCurves,
    rangeMillis,
    numeric,
    type Range,
    type Curve,
  } from '$lib/metrics';
  import { seriesColor, normalizeState } from '$lib/charts';
  import { platformLabel } from '$lib/format';
  import StatusMark from '$lib/components/StatusMark.svelte';
  import BufferBadge from '$lib/components/BufferBadge.svelte';
  import ChartCard from '$lib/components/ChartCard.svelte';
  import TimeSeriesChart from '$lib/components/TimeSeriesChart.svelte';
  import StatusBand from '$lib/components/StatusBand.svelte';
  import SeriesLegend from '$lib/components/SeriesLegend.svelte';
  import CommandButton from '$lib/components/CommandButton.svelte';
  import ValueTable from '$lib/components/ValueTable.svelte';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import ActionMenu from '$lib/components/ActionMenu.svelte';
  import CodeDisplay from '$lib/components/CodeDisplay.svelte';
  import { enrollments } from '$lib/enroll';
  import type { EnrollCode } from '$lib/api';

  const { id, onExpired } = $props<{ id: string; onExpired: () => void }>();

  const lang = $derived($locale ?? 'en');
  const probe = $derived($fleet.probes.find((p) => p.probe_id === id) ?? null);

  // ── Rafraîchissement automatique ─────────────────────────────────────────
  //
  // Même raison que dans le parc : le statut est dérivé du dernier battement,
  // et une fiche laissée ouverte afficherait un « en ligne » vieux d'une
  // heure. Ici les graphiques suivent aussi, sinon on regarde une courbe qui
  // s'arrête sans que rien ne le dise.
  //
  // ⚠️ `quiet` sur les deux : un rechargement visible ferait clignoter la page
  // toutes les trente secondes. La barre de rafraîchissement s'affiche déjà
  // par `refreshing`, qui garde le rendu précédent pendant la lecture.
  // ── Onglets (contrat § 14) ───────────────────────────────────────────────
  //
  // La fiche reprend ce que fait LanProbe et permet de le déclencher à
  // distance. L'onglet vit dans le hash : recharger la page, ou envoyer le
  // lien à quelqu'un, doit rouvrir la même vue.
  const TABS = [
    'dashboard',
    'monitoring',
    'speedtest',
    'ports',
    'discovery',
    'commands',
  ] as const;
  type Tab = (typeof TABS)[number];

  let tab = $state<Tab>('dashboard');

  const REFRESH_MS = 30_000;
  let timer: ReturnType<typeof setInterval> | null = null;

  onMount(() => {
    // Accès direct par URL : le parc n'est peut-être pas encore chargé.
    if ($fleet.probes.length === 0) void fleet.load(onExpired);
    timer = setInterval(() => {
      void fleet.load(onExpired, { quiet: true });
      void loadMetrics();
    }, REFRESH_MS);
  });

  onDestroy(() => {
    if (timer) clearInterval(timer);
  });

  // ── Renommage en place ───────────────────────────────────────────────────
  let editing = $state(false);
  let draft = $state('');
  let nameBusy = $state(false);
  let nameError = $state('');

  function startEdit() {
    draft = probe?.name ?? '';
    nameError = '';
    editing = true;
  }

  async function saveName() {
    if (!probe) return;
    const name = draft.trim();
    if (!name) return void (nameError = $_('probe.rename_empty'));
    if (name === probe.name) return void (editing = false);
    nameBusy = true;
    nameError = '';
    try {
      await api.patchProbe(probe.probe_id, { name });
      fleet.patchLocal({ ...probe, name });
      editing = false;
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      nameError =
        e instanceof ApiError && e.isConflict
          ? $_('probe.rename_conflict', { values: { name, site: probe.site } })
          : e instanceof ApiError
            ? e.message
            : String(e);
    } finally {
      nameBusy = false;
    }
  }

  // ── Déplacement de site ──────────────────────────────────────────────────
  let moveOpen = $state(false);
  let moveTo = $state('');
  let moveBusy = $state(false);
  let moveError = $state('');

  function openMove() {
    moveTo = probe?.site_id ?? '';
    moveError = '';
    moveOpen = true;
  }

  async function submitMove() {
    if (!probe || !moveTo || moveTo === probe.site_id) return void (moveOpen = false);
    moveBusy = true;
    moveError = '';
    const target = $fleet.sites.find((s) => s.site_id === moveTo);
    try {
      await api.patchProbe(probe.probe_id, { site_id: moveTo });
      moveOpen = false;
      await fleet.load(onExpired, { quiet: true });
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      moveError =
        e instanceof ApiError && e.isConflict
          ? $_('probe.move_conflict', {
              values: { site: target?.name ?? '', name: probe.name },
            })
          : e instanceof ApiError
            ? e.message
            : String(e);
    } finally {
      moveBusy = false;
    }
  }

  // ── Révocation ───────────────────────────────────────────────────────────
  let revokeOpen = $state(false);
  let revokeBusy = $state(false);
  let revokeError = $state('');

  async function submitRevoke() {
    if (!probe) return;
    revokeBusy = true;
    revokeError = '';
    try {
      await api.revoke(probe.probe_id);
      flash.set($_('revoke.done', { values: { name: probe.name } }));
      fleet.dropLocal(probe.probe_id);
      revokeOpen = false;
      go('/');
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      revokeError = e instanceof ApiError ? e.message : String(e);
    } finally {
      revokeBusy = false;
    }
  }

  // ── Clé d'écriture : trois gestes, trois conséquences ────────────────────
  const hubUrl = `${location.protocol}//${location.host}`;

  // Rotation d'hygiène : sans conséquence immédiate, donc sans confirmation.
  // Le drapeau local rend l'état intermédiaire visible tout de suite, même si
  // `GET /api/probes` ne le renvoie pas encore.
  let rotateBusy = $state(false);
  let rotateDone = $state(false);
  let keyError = $state('');

  const pending = $derived(rotateDone || probe?.token_rotation_pending === true);

  async function rotate() {
    if (!probe) return;
    rotateBusy = true;
    keyError = '';
    try {
      await api.rotateToken(probe.probe_id);
      rotateDone = true;
      await fleet.load(onExpired, { quiet: true });
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      keyError = e instanceof ApiError ? e.message : String(e);
    } finally {
      rotateBusy = false;
    }
  }

  // Révocation immédiate : c'est la seule des trois qui coupe une sonde, donc
  // la seule à demander une confirmation. Tout confirmer reviendrait à ne
  // rien faire lire.
  let killOpen = $state(false);
  let killBusy = $state(false);
  let killDone = $state('');

  async function killToken() {
    if (!probe) return;
    killBusy = true;
    keyError = '';
    try {
      await api.revokeToken(probe.probe_id);
      killOpen = false;
      killDone = $_('probe.revoke_now_done');
      await fleet.load(onExpired, { quiet: true });
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      keyError = e instanceof ApiError ? e.message : String(e);
      killOpen = false;
    } finally {
      killBusy = false;
    }
  }

  // Ré-enrôlement : émet un code, ne casse rien — pas de confirmation.
  let reOpen = $state(false);
  let reBusy = $state(false);
  let reCode = $state<EnrollCode | null>(null);
  let reError = $state('');

  async function reenroll() {
    if (!probe) return;
    reBusy = true;
    reError = '';
    reOpen = true;
    try {
      reCode = await api.reenrollCode(probe.probe_id);
      // La même attente s'affiche aussi dans le parc, sur la ligne du site :
      // sans ça, on quitterait cet écran et le code paraîtrait perdu.
      enrollments.remember(
        {
          site_id: probe.site_id,
          site: probe.site,
          probe_id: probe.probe_id,
          probe: probe.name,
          created_at: Math.floor(Date.now() / 1000),
          expires_at: reCode.expires_at,
        },
        reCode.code,
      );
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      reError = e instanceof ApiError ? e.message : String(e);
    } finally {
      reBusy = false;
    }
  }

  // ── Mesures ──────────────────────────────────────────────────────────────
  interface ChartState {
    tone: 'loading' | 'error' | 'empty' | 'ready';
    error?: string;
    curves: Curve[];
  }
  const blank: ChartState = { tone: 'loading', curves: [] };

  let range = $state<Range>(DEFAULT_RANGE);
  let ping = $state<ChartState>({ ...blank });
  let inet = $state<ChartState>({ ...blank });
  let speed = $state<ChartState>({ ...blank });
  let refreshing = $state(false);
  /**
   * Instant de la dernière lecture : c'est lui qui borne l'axe, pas `now`.
   * Une horloge réactive ferait glisser les trois graphiques à chaque
   * seconde alors que leurs points, eux, ne bougent pas.
   */
  let loadedAt = $state(Date.now());

  async function fetchOne(measurement: string, fallbackField: string): Promise<ChartState> {
    try {
      const raw = await api.metrics(id, measurement, range);
      const curves = normalizeCurves(raw, fallbackField);
      const empty = curves.every((c) => c.points.length === 0);
      return { tone: empty ? 'empty' : 'ready', curves };
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) {
        onExpired();
        return { tone: 'error', curves: [], error: $_('auth.expired') };
      }
      if (e instanceof MetricsShapeError) {
        // On nomme les clés reçues : le message doit permettre de corriger le
        // backend, pas seulement de constater l'échec.
        return {
          tone: 'error',
          curves: [],
          error: $_('charts.error_shape', { values: { keys: e.observed } }),
        };
      }
      return {
        tone: 'error',
        curves: [],
        error: e instanceof ApiError ? e.message : String(e),
      };
    }
  }

  async function loadMetrics() {
    const hadData = ping.tone === 'ready' || inet.tone === 'ready' || speed.tone === 'ready';
    refreshing = hadData;
    if (!hadData) {
      ping = { ...blank };
      inet = { ...blank };
      speed = { ...blank };
    }
    const [p, i, s] = await Promise.all([
      fetchOne(MEASUREMENT.ping, 'latency_ms'),
      fetchOne(MEASUREMENT.internet, 'state'),
      fetchOne(MEASUREMENT.speedtest, 'download_mbps'),
    ]);
    ping = p;
    inet = i;
    speed = s;
    loadedAt = Date.now();
    refreshing = false;
  }

  // Recharge à chaque changement de fenêtre ou de sonde — la barre de filtres
  // cadre les trois graphiques à la fois, jamais un seul.
  // `untrack` est indispensable : loadMetrics lit ping/inet/speed pour savoir
  // s'il doit garder le rendu précédent, et les réécrit ensuite. Sans lui,
  // l'effet se redéclencherait sur sa propre écriture.
  $effect(() => {
    void range;
    void id;
    untrack(() => void loadMetrics());
  });

  // La fenêtre demandée borne l'axe, même si les mesures n'en couvrent qu'un
  // coin : « 6 h » doit montrer six heures, sinon trente minutes de relevés
  // occupent toute la largeur et on croit lire six heures de données.
  const domain = $derived({ from: loadedAt - rangeMillis(range), to: loadedAt });

  // ── Inventaire (contrat § 12) ────────────────────────────────────────────
  //
  // Chargé à la demande, pas au montage : trois requêtes de plus à chaque
  // ouverture de fiche pour des onglets qu'on ne consulte pas toujours.
  let portsScan = $state<Scan | null | undefined>(undefined);
  let discoveryScan = $state<Scan | null | undefined>(undefined);
  let speedHistory = $state<SpeedtestRow[] | undefined>(undefined);
  let commandLog = $state<ProbeCommand[]>([]);
  let inventoryError = $state('');

  async function loadInventory(kind: 'ports' | 'discovery' | 'speedtest') {
    inventoryError = '';
    try {
      if (kind === 'speedtest') {
        const r = await api.inventory<{ speedtests: SpeedtestRow[] }>(id, 'speedtest');
        speedHistory = r.speedtests;
      } else {
        const r = await api.inventory<{ scan: Scan | null }>(id, kind);
        if (kind === 'ports') portsScan = r.scan;
        else discoveryScan = r.scan;
      }
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      inventoryError = e instanceof ApiError ? e.message : String(e);
    }
  }

  async function loadCommands() {
    try {
      commandLog = (await api.commands(id)).commands;
    } catch {
      /* le journal des commandes est un confort : son absence n'empêche rien */
    }
  }

  // Recharge ce que l'onglet ouvert a besoin de montrer, et lui seul.
  $effect(() => {
    const current = tab;
    void id;
    untrack(() => {
      if (current === 'ports') void loadInventory('ports');
      else if (current === 'discovery') void loadInventory('discovery');
      else if (current === 'speedtest') void loadInventory('speedtest');
      if (current === 'commands') void loadCommands();
    });
  });

  /**
   * ⚠️ Une commande n'agit qu'au prochain battement. Recharger juste après
   * l'envoi ne montrerait rien de neuf — c'est le journal des commandes qui
   * rend l'attente visible, pas la liste des cibles.
   */
  function afterCommand() {
    void loadCommands();
  }

  // Une courbe par cible pinguée : le hub les sépare déjà, les rassembler
  // dessinerait une latence moyenne qu'aucune machine n'a jamais mesurée.
  const pingCurves = $derived.by(() => {
    const latency = ping.curves.filter((c) => c.field === 'latency_ms' && c.points.length > 0);
    return latency.map((c, i) => ({
      key: c.label,
      // Une seule cible : pas la peine d'afficher son adresse en légende.
      label: latency.length > 1 ? (c.tags.ip ?? c.label) : $_('charts.latency'),
      color: seriesColor(i),
      points: numeric(c.points),
    }));
  });
  const pingPoints = $derived(pingCurves.flatMap((c) => c.points));

  /** Cibles ICMP surveillées, telles que les mesures les nomment. */
  const monitoredTargets = $derived(
    [...new Set(pingCurves.map((c) => c.key.replace(/^latency_ms\s*·\s*/, '')))].sort(),
  );

  let newTarget = $state('');
  const inetPoints = $derived(inet.curves.find((c) => c.field === 'state')?.points ?? []);

  // ── Débits ───────────────────────────────────────────────────────────────
  //
  // Une sonde peut mesurer avec deux moteurs — Ookla vers internet, iperf3
  // vers un serveur du LAN. Ce ne sont pas les mêmes ordres de grandeur ni la
  // même question, mais on veut les comparer d'un coup d'œil : une courbe par
  // (sens, moteur) sur le même graphique, jamais une fusion qui ferait croire
  // à un débit unique.
  const ENGINE_LABEL: Record<string, string> = {
    ookla: 'Speedtest.net',
    iperf3: 'iperf3',
  };
  function engineLabel(raw: string | undefined): string {
    if (!raw) return '';
    return ENGINE_LABEL[raw.toLowerCase()] ?? raw.charAt(0).toUpperCase() + raw.slice(1);
  }

  const speedCurves = $derived.by(() => {
    const wanted: [string, string][] = [
      ['download_mbps', $_('charts.download')],
      ['upload_mbps', $_('charts.upload')],
    ];
    const engines = new Set(
      speed.curves.filter((c) => c.points.length > 0).map((c) => c.tags.engine ?? ''),
    );
    const out: { key: string; label: string; color: string; points: { t: number; v: number }[] }[] = [];
    for (const [field, direction] of wanted) {
      for (const c of speed.curves.filter((c) => c.field === field && c.points.length > 0)) {
        const engine = engineLabel(c.tags.engine);
        out.push({
          key: `${field}:${c.tags.engine ?? ''}`,
          // Le moteur n'apparaît que s'il y en a plusieurs : sur une sonde qui
          // n'en utilise qu'un, « Descendant · Speedtest.net » est du bruit.
          label: engines.size > 1 && engine ? `${direction} · ${engine}` : direction,
          color: seriesColor(out.length),
          points: numeric(c.points),
        });
      }
    }
    return out;
  });

  /** Moteurs effectivement représentés, pour l'étiquette de l'en-tête. */
  const speedEngines = $derived([
    ...new Set(
      speed.curves
        .filter((c) => c.points.length > 0 && c.tags.engine)
        .map((c) => engineLabel(c.tags.engine)),
    ),
  ]);

  /** Serveur du dernier test — « 216 Mbit/s » ne dit rien sans lui. */
  const lastServer = $derived.by(() => {
    const curve = speed.curves.find((c) => c.field === 'server' && c.points.length > 0);
    const last = curve?.points[curve.points.length - 1]?.v;
    return typeof last === 'string' && last.trim() ? last : null;
  });

  /**
   * Lien vers le résultat public du dernier test Ookla. iperf3 n'en produit
   * pas — l'absence de lien est donc une information, pas une panne.
   */
  const lastResultUrl = $derived.by(() => {
    const curve = speed.curves.find((c) => c.field === 'result_url' && c.points.length > 0);
    const last = curve?.points[curve.points.length - 1]?.v;
    return typeof last === 'string' && /^https?:\/\//.test(last) ? last : null;
  });

  // ── Tableaux de valeurs ──────────────────────────────────────────────────
  //
  // Les tableaux se lisent par instant, pas par courbe : on regroupe donc sur
  // l'horodatage. Sans ça, deux cibles pinguées donnaient deux lignes
  // interlignées qu'on prenait pour des relevés successifs d'une seule.
  const pingRows = $derived.by(() => {
    const rows = pingCurves.flatMap((c) =>
      c.points.map((p) => ({ t: p.t, target: c.label, v: p.v })),
    );
    rows.sort((a, b) => a.t - b.t || a.target.localeCompare(b.target));
    return rows.map((r) => [tooltipTime(r.t, lang), r.target, r.v.toFixed(0)]);
  });

  const speedRows = $derived.by(() => {
    const byRun = new Map<string, { t: number; engine: string; dl?: number; ul?: number }>();
    for (const c of speed.curves) {
      if (c.field !== 'download_mbps' && c.field !== 'upload_mbps') continue;
      const engine = engineLabel(c.tags.engine);
      for (const p of numeric(c.points)) {
        const key = `${p.t}:${engine}`;
        const row = byRun.get(key) ?? { t: p.t, engine };
        if (c.field === 'download_mbps') row.dl = p.v;
        else row.ul = p.v;
        byRun.set(key, row);
      }
    }
    return [...byRun.values()]
      .sort((a, b) => a.t - b.t)
      .map((r) => [
        tooltipTime(r.t, lang),
        r.engine || '—',
        r.dl != null ? r.dl.toFixed(1) : '—',
        r.ul != null ? r.ul.toFixed(1) : '—',
      ]);
  });

  const speedTone = $derived(
    speed.tone === 'ready' && speedCurves.every((c) => c.points.length === 0)
      ? 'empty'
      : speed.tone,
  );
  const pingTone = $derived(
    ping.tone === 'ready' && pingPoints.length === 0 ? 'empty' : ping.tone,
  );
  const inetTone = $derived(
    inet.tone === 'ready' && inetPoints.length === 0 ? 'empty' : inet.tone,
  );

  /**
   * Courbes masquées à la main, par clé. Plusieurs cibles pinguées se
   * superposent vite : pouvoir en éteindre une rend les autres lisibles sans
   * changer la fenêtre ni recharger.
   *
   * L'état est volontairement local à la page : il vaut pour un coup d'œil,
   * pas pour une préférence. Le retenir ferait revenir sur une sonde avec des
   * courbes manquantes sans qu'on se souvienne de les avoir éteintes.
   */
  let hidden = $state(new SvelteSet<string>());
  function toggle(key: string) {
    if (hidden.has(key)) hidden.delete(key);
    else hidden.add(key);
  }

  const shownPing = $derived(pingCurves.filter((c) => !hidden.has(c.key)));
  const shownSpeed = $derived(speedCurves.filter((c) => !hidden.has(c.key)));


</script>

<a class="back" href="#/">
  <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
    <polyline points="10,3 5,8 10,13" />
  </svg>
  {$_('probe.back')}
</a>

{#if !probe}
  {#if $fleet.loading}
    <StateBlock tone="loading" title={$_('fleet.loading')} />
  {:else}
    <StateBlock
      tone="error"
      title={$_('probe.not_found_title')}
      body={$_('probe.not_found_body')}
    >
      <button class="lp-btn primary" onclick={() => go('/')}>{$_('probe.back')}</button>
    </StateBlock>
  {/if}
{:else}
  <header class="head">
    <div class="ident">
      {#if editing}
        <div class="edit">
          <input
            class="lp-input"
            bind:value={draft}
            aria-label={$_('probe.rename_label')}
            onkeydown={(e) => {
              if (e.key === 'Enter') saveName();
              if (e.key === 'Escape') editing = false;
            }}
          />
          <button class="lp-btn primary sm" onclick={saveName} disabled={nameBusy}>
            {$_('common.save')}
          </button>
          <button class="lp-btn sm" onclick={() => (editing = false)}>{$_('common.cancel')}</button>
        </div>
        {#if nameError}<p class="err" role="alert">{nameError}</p>{/if}
      {:else}
        <div class="title-row">
          <h1>{probe.name}</h1>
          <button class="lp-btn ghost sm" onclick={startEdit}>{$_('probe.rename')}</button>
        </div>
      {/if}

      <div class="badges">
        <StatusMark status={probe.status} />
        <span class="dot" aria-hidden="true">·</span>
        <span class="site">{probe.site}</span>
        <span class="dot" aria-hidden="true">·</span>
        <span
          class="seen lp-mono"
          title={probe.last_seen ? absoluteTime(probe.last_seen, lang) : $_('status.never_seen')}
        >
          {probe.last_seen ? relativeTime(probe.last_seen, lang, $now) : $_('status.never_seen')}
        </span>
      </div>
    </div>

    <!--
      Toutes les actions de la fiche sont derrière un seul engrenage. Aucune
      n'est courante — on vient ici pour lire des courbes, pas pour révoquer une
      clé — et posées en clair dans la page elles étaient atteignables par un
      simple défilement. Les explications ne disparaissent pas : chaque item
      porte la sienne, en toutes lettres, avant le clic.
    -->
    <div class="acts">
      <ActionMenu label={$_('probe.actions')}>
        {#snippet children(close)}
          <p class="mgroup">{$_('probe.menu_probe')}</p>
          <button
            class="mi"
            role="menuitem"
            onclick={() => {
              close();
              openMove();
            }}
          >
            <span class="mi-label">{$_('probe.move')}</span>
            <span class="mi-hint">{$_('probe.move_hint')}</span>
          </button>

          <p class="mgroup">{$_('probe.keys_title')}</p>
          <p class="mintro">{$_('probe.keys_intro')}</p>

          <button
            class="mi"
            role="menuitem"
            disabled={rotateBusy}
            onclick={() => {
              close();
              void rotate();
            }}
          >
            <span class="mi-label">{rotateBusy ? $_('probe.rotating') : $_('probe.rotate')}</span>
            <span class="mi-hint">{$_('probe.rotate_hint')}</span>
          </button>

          <button
            class="mi accent"
            role="menuitem"
            disabled={reBusy}
            onclick={() => {
              close();
              void reenroll();
            }}
          >
            <span class="mi-label">
              {reBusy ? $_('probe.reenroll_working') : $_('probe.reenroll')}
            </span>
            <span class="mi-hint">{$_('probe.reenroll_hint')}</span>
          </button>

          <div class="mdanger">
            <button
              class="mi danger"
              role="menuitem"
              onclick={() => {
                close();
                killOpen = true;
              }}
            >
              <span class="mi-label">{$_('probe.revoke_now')}</span>
              <span class="mi-hint">{$_('probe.revoke_now_hint')}</span>
            </button>

            <button
              class="mi danger"
              role="menuitem"
              onclick={() => {
                close();
                revokeOpen = true;
              }}
            >
              <span class="mi-label">{$_('revoke.action')}</span>
              <span class="mi-hint">{$_('revoke.keeps_data')}</span>
            </button>
          </div>
        {/snippet}
      </ActionMenu>
    </div>
  </header>

  <!--
    Ce que les actions du menu ont laissé derrière elles. C'est un état de la
    sonde, pas une commande : sa place est près de l'identité, avec l'alerte de
    tampon, et non dans un bloc d'administration en bas de page.
  -->
  {#if pending}
    <div class="pending" role="status">
      <strong>{$_('probe.pending_title')}</strong>
      <p>{$_('probe.pending_body')}</p>
    </div>
  {/if}
  {#if killDone}
    <p class="done" role="status">{killDone}</p>
  {/if}
  {#if keyError}
    <p class="err top" role="alert">{keyError}</p>
  {/if}

  <dl class="meta">
    <div><dt>{$_('probe.meta_id')}</dt><dd class="lp-mono id">{probe.probe_id}</dd></div>
    <div><dt>{$_('probe.meta_site')}</dt><dd>{probe.site}</dd></div>
    <div><dt>{$_('probe.meta_platform')}</dt><dd class="lp-mono">{platformLabel(probe.platform, '—')}</dd></div>
    <div><dt>{$_('probe.meta_version')}</dt><dd class="lp-mono">{probe.version || '—'}</dd></div>
    <div>
      <dt>{$_('probe.meta_created')}</dt>
      <dd class="lp-mono">{dateOnly(probe.created_at, lang)}</dd>
    </div>
    <div>
      <dt>{$_('buffer.label')}</dt>
      <dd><BufferBadge points={probe.buffered_points} /></dd>
    </div>
  </dl>

  {#if probe.buffered_points > 0}
    <div class="buffer-alert" role="status">
      <strong>{$_('buffer.pending', { values: { n: probe.buffered_points } })}</strong>
      <p>{$_('buffer.warn_body')}</p>
    </div>
  {/if}

  <nav class="tabs" aria-label={$_('probe.tabs_aria')}>
    {#each TABS as t (t)}
      <button class="tab" class:on={tab === t} onclick={() => (tab = t)}>
        {$_(`probe.tab_${t}`)}
      </button>
    {/each}
  </nav>

  {#if tab === 'dashboard'}
  <div class="rangebar">
    <span class="lp-label">{$_('probe.range')}</span>
    <div class="chips" role="group" aria-label={$_('probe.range')}>
      {#each RANGES as r (r)}
        <button class="chip" class:on={range === r} onclick={() => (range = r)}>
          {$_(`probe.range_${r.replace('-', '')}`)}
        </button>
      {/each}
    </div>
  </div>

  <div class="charts">
    <ChartCard
      title={$_('charts.latency_title')}
      sub={$_('charts.latency_sub')}
      tone={pingTone}
      errorText={ping.error}
      {refreshing}
    >
      <TimeSeriesChart
        series={shownPing}
        {domain}
        unit={$_('charts.unit_ms')}
      />
      <SeriesLegend
        rows={pingCurves}
        unit={$_('charts.unit_ms')}
        decimals={0}
        {hidden}
        ontoggle={toggle}
      />
      {#snippet table()}
        <ValueTable
          columns={[
            $_('charts.table_time'),
            $_('charts.table_target'),
            `${$_('charts.latency')} (${$_('charts.unit_ms')})`,
          ]}
          rows={pingRows}
        />
      {/snippet}
    </ChartCard>

    <ChartCard
      title={$_('charts.internet_title')}
      sub={$_('charts.internet_sub')}
      tone={inetTone}
      errorText={inet.error}
      {refreshing}
    >
      <StatusBand points={inetPoints} {domain} />
      {#snippet table()}
        <ValueTable
          columns={[$_('charts.table_time'), $_('charts.table_state')]}
          rows={inetPoints.map((p) => [
            tooltipTime(p.t, lang),
            $_(`charts.internet_${normalizeState(p.v)}`),
          ])}
        />
      {/snippet}
    </ChartCard>

    <ChartCard
      title={$_('charts.speed_title')}
      sub={$_('charts.speed_sub')}
      tone={speedTone}
      errorText={speed.error}
      {refreshing}
    >
      {#snippet action()}
        {#if speedEngines.length}
          <span class="engine">{speedEngines.join(' · ')}</span>
        {/if}
        {#if lastResultUrl}
          <a class="lp-btn ghost sm" href={lastResultUrl} target="_blank" rel="noopener noreferrer">
            {$_('charts.speed_result_link')}
          </a>
        {/if}
      {/snippet}
      <TimeSeriesChart series={shownSpeed} unit={$_('charts.unit_mbps')} decimals={1} {domain} />
      <SeriesLegend
        rows={speedCurves}
        unit={$_('charts.unit_mbps')}
        {hidden}
        ontoggle={toggle}
      />
      {#if lastServer}
        <p class="server lp-mono">{$_('charts.speed_server')} {lastServer}</p>
      {/if}
      {#snippet table()}
        <ValueTable
          columns={[
            $_('charts.table_time'),
            $_('charts.table_engine'),
            `${$_('charts.download')} (${$_('charts.unit_mbps')})`,
            `${$_('charts.upload')} (${$_('charts.unit_mbps')})`,
          ]}
          rows={speedRows}
        />
      {/snippet}
    </ChartCard>
  </div>
  {/if}

  {#if tab !== 'dashboard'}
    <div class="panel">
      {#if inventoryError}
        <StateBlock tone="error" title={$_('charts.error')} body={inventoryError} />
      {/if}

      {#if tab === 'monitoring'}
        <section class="lp-card block">
          <header class="bh">
            <div>
              <h2 class="lp-title">{$_('probe.tab_monitoring')}</h2>
              <p class="bsub">{$_('probe.monitoring_sub')}</p>
            </div>
          </header>

          <!-- Les cibles viennent des mesures elles-mêmes : c'est la seule
               source qui dit ce que la sonde surveille VRAIMENT, plutôt que ce
               qu'on lui a demandé de surveiller un jour. -->
          {#if monitoredTargets.length === 0}
            <p class="empty">{$_('probe.monitoring_none')}</p>
          {:else}
            <ul class="targets">
              {#each monitoredTargets as ip (ip)}
                <li>
                  <span class="lp-mono">{ip}</span>
                  <CommandButton
                    probeId={id}
                    kind="remove_monitor"
                    args={{ ip }}
                    label={$_('probe.monitoring_remove')}
                    onsent={afterCommand}
                  />
                </li>
              {/each}
            </ul>
          {/if}

          <form class="addrow" onsubmit={(e) => e.preventDefault()}>
            <input
              class="lp-input"
              bind:value={newTarget}
              placeholder={$_('probe.monitoring_placeholder')}
              spellcheck="false"
              autocomplete="off"
            />
            {#if newTarget.trim()}
              <CommandButton
                probeId={id}
                kind="add_monitor"
                args={{ ip: newTarget.trim() }}
                label={$_('probe.monitoring_add')}
                onsent={() => { newTarget = ''; afterCommand(); }}
              />
            {/if}
          </form>
        </section>
      {:else if tab === 'speedtest'}
        <section class="lp-card block">
          <header class="bh">
            <div>
              <h2 class="lp-title">{$_('probe.tab_speedtest')}</h2>
              <p class="bsub">{$_('probe.speedtest_sub')}</p>
            </div>
            <CommandButton
              probeId={id}
              kind="speedtest"
              label={$_('probe.speedtest_run')}
              onsent={afterCommand}
            />
          </header>

          {#if speedHistory === undefined}
            <p class="empty">{$_('charts.loading')}</p>
          {:else if speedHistory.length === 0}
            <p class="empty">{$_('probe.speedtest_none')}</p>
          {:else}
            <div class="tablewrap">
              <table class="grid">
                <thead>
                  <tr>
                    <th>{$_('charts.table_time')}</th>
                    <th>{$_('charts.table_engine')}</th>
                    <th>{$_('charts.speed_server')}</th>
                    <th class="num">{$_('charts.download')}</th>
                    <th class="num">{$_('charts.upload')}</th>
                    <th class="num">{$_('charts.latency')}</th>
                    <th></th>
                  </tr>
                </thead>
                <tbody>
                  {#each speedHistory as r (r.started_at)}
                    <tr>
                      <td class="lp-mono">{logTime(r.started_at, lang)}</td>
                      <td>{r.engine ?? '—'}</td>
                      <td class="srv">{r.server_name ?? '—'}</td>
                      <td class="num lp-mono">{r.download_mbps?.toFixed(1) ?? '—'}</td>
                      <td class="num lp-mono">{r.upload_mbps?.toFixed(1) ?? '—'}</td>
                      <td class="num lp-mono">{r.latency_ms ?? '—'}</td>
                      <td>
                        {#if r.result_url}
                          <a href={r.result_url} target="_blank" rel="noopener noreferrer">
                            {$_('charts.speed_result_link')}
                          </a>
                        {/if}
                      </td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        </section>
      {:else if tab === 'ports'}
        <section class="lp-card block">
          <header class="bh">
            <div>
              <h2 class="lp-title">{$_('probe.tab_ports')}</h2>
              <p class="bsub">{$_('probe.ports_sub')}</p>
            </div>
            <form class="inline" onsubmit={(e) => e.preventDefault()}>
              <input
                class="lp-input"
                bind:value={newTarget}
                placeholder={$_('probe.ports_placeholder')}
                spellcheck="false"
                autocomplete="off"
              />
              {#if newTarget.trim()}
                <CommandButton
                  probeId={id}
                  kind="port_scan"
                  args={{ ip: newTarget.trim() }}
                  label={$_('probe.ports_run')}
                  onsent={afterCommand}
                />
              {/if}
            </form>
          </header>

          {#if portsScan === undefined}
            <p class="empty">{$_('charts.loading')}</p>
          {:else if portsScan === null}
            <!-- ⚠️ « Jamais lancé » et « lancé, rien trouvé » ne se disent pas
                 pareil : le second est une conclusion sur le réseau. -->
            <p class="empty">{$_('probe.ports_never')}</p>
          {:else}
            <p class="scanmeta">{$_('probe.scan_at', { values: { when: logTime(portsScan.started_at, lang) } })}</p>
            {#if portsScan.ports.length === 0}
              <p class="empty">{$_('probe.ports_empty')}</p>
            {:else}
              <div class="tablewrap">
                <table class="grid">
                  <thead>
                    <tr>
                      <th>{$_('probe.col_ip')}</th>
                      <th class="num">{$_('probe.col_port')}</th>
                      <th>{$_('probe.col_proto')}</th>
                      <th>{$_('probe.col_service')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each portsScan.ports as p (`${p.ip}:${p.port}/${p.proto}`)}
                      <tr>
                        <td class="lp-mono">{p.ip}</td>
                        <td class="num lp-mono">{p.port}</td>
                        <td class="lp-mono">{p.proto}</td>
                        <td>{p.service ?? '—'}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            {/if}
          {/if}
        </section>
      {:else if tab === 'discovery'}
        <section class="lp-card block">
          <header class="bh">
            <div>
              <h2 class="lp-title">{$_('probe.tab_discovery')}</h2>
              <p class="bsub">{$_('probe.discovery_sub')}</p>
            </div>
            <CommandButton
              probeId={id}
              kind="discovery"
              label={$_('probe.discovery_run')}
              onsent={afterCommand}
            />
          </header>

          {#if discoveryScan === undefined}
            <p class="empty">{$_('charts.loading')}</p>
          {:else if discoveryScan === null}
            <p class="empty">{$_('probe.discovery_never')}</p>
          {:else}
            <p class="scanmeta">
              {$_('probe.scan_at', { values: { when: logTime(discoveryScan.started_at, lang) } })}
              {#if discoveryScan.cidr}<span class="lp-mono"> · {discoveryScan.cidr}</span>{/if}
            </p>
            {#if discoveryScan.hosts.length === 0}
              <p class="empty">{$_('probe.discovery_empty')}</p>
            {:else}
              <div class="tablewrap">
                <table class="grid">
                  <thead>
                    <tr>
                      <th>{$_('probe.col_ip')}</th>
                      <th>{$_('probe.col_hostname')}</th>
                      <th>{$_('probe.col_mac')}</th>
                      <th>{$_('probe.col_vendor')}</th>
                      <th class="num">{$_('charts.latency')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each discoveryScan.hosts as h (h.ip)}
                      <tr>
                        <td class="lp-mono">{h.ip}</td>
                        <td>{h.hostname ?? '—'}</td>
                        <td class="lp-mono">{h.mac ?? '—'}</td>
                        <td>{h.vendor ?? '—'}</td>
                        <td class="num lp-mono">{h.latency_ms ?? '—'}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            {/if}
          {/if}
        </section>
      {:else if tab === 'commands'}
        <!--
          ⚠️ Le journal a son propre onglet plutôt que d'être répété sous les
          quatre autres. Répété, il donnait quatre fois la même table et
          allongeait chaque vue d'un bloc qu'on ne lit qu'en cas de doute.

          Le retour immédiat après un clic, lui, n'est pas perdu : c'est le
          bouton qui l'annonce, sur place — « en file, la sonde l'exécutera à
          son prochain battement ». C'est ce message qui empêche de recliquer,
          pas la table.
        -->
        <section class="lp-card block">
          <header class="bh">
            <div>
              <h2 class="lp-title">{$_('commands.title')}</h2>
              <p class="bsub">{$_('commands.sub')}</p>
            </div>
          </header>
          {#if commandLog.length === 0}
            <p class="empty">{$_('commands.none')}</p>
          {:else}
            <div class="tablewrap">
              <table class="grid">
                <thead>
                  <tr>
                    <th>{$_('commands.col_when')}</th>
                    <th>{$_('commands.col_kind')}</th>
                    <th>{$_('commands.col_target')}</th>
                    <th>{$_('commands.col_state')}</th>
                    <th>{$_('commands.col_by')}</th>
                  </tr>
                </thead>
                <tbody>
                  {#each commandLog as c (c.id)}
                    <tr>
                      <td class="lp-mono">{logTime(c.created_at, lang)}</td>
                      <td>{$_(`commands.kind_${c.kind}`)}</td>
                      <td class="lp-mono">{typeof c.args?.ip === 'string' ? c.args.ip : '—'}</td>
                      <td>
                        <span class="cstate {c.state}">{$_(`commands.state_${c.state}`)}</span>
                        {#if c.state === 'pending' && c.delivered_count > 0}
                          <span class="cnote">{$_('commands.deliveries', { values: { n: c.delivered_count } })}</span>
                        {:else if c.error}
                          <span class="cnote">{c.error}</span>
                        {/if}
                      </td>
                      <td>{c.created_by ?? '—'}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        </section>
      {/if}
    </div>
  {/if}

  <Modal
    open={killOpen}
    title={$_('probe.revoke_now_title', { values: { name: probe.name } })}
    onclose={() => (killOpen = false)}
  >
    <!--
      Le tri « fuite de clé ou machine compromise ? » est posé ici et pas
      ailleurs : c'est le dernier instant où les deux issues sont encore
      ouvertes. Quelqu'un qui reconnaît le second cas annule et ré-enrôle.
    -->
    <div class="triage">
      <h3>{$_('probe.which_title')}</h3>
      <ul>
        <li>{$_('probe.which_leak')}</li>
        <li>{$_('probe.which_host')}</li>
      </ul>
    </div>
    <p>{$_('probe.revoke_now_body')}</p>
    <p class="keep"><strong>{$_('probe.revoke_now_keeps')}</strong></p>
    {#snippet footer()}
      <button class="lp-btn" onclick={() => (killOpen = false)}>{$_('common.cancel')}</button>
      <button class="lp-btn danger" onclick={killToken} disabled={killBusy}>
        {killBusy ? $_('probe.revoke_now_working') : $_('probe.revoke_now_confirm')}
      </button>
    {/snippet}
  </Modal>

  <Modal
    open={reOpen}
    wide
    title={$_('probe.reenroll_title', { values: { name: probe.name } })}
    onclose={() => (reOpen = false)}
  >
    <p class="keep"><strong>{$_('probe.reenroll_keeps')}</strong></p>
    {#if reError}
      <p class="err" role="alert">{reError}</p>
    {:else if reCode}
      <CodeDisplay
        code={reCode.code}
        expiresAt={reCode.expires_at}
        {hubUrl}
        dictate={$_('code.dictate_reenroll')}
        onregenerate={reenroll}
        busy={reBusy}
        regenerateLabel={$_('code.regenerate')}
      />
    {:else}
      <p class="muted">{$_('probe.reenroll_working')}</p>
    {/if}
    {#snippet footer()}
      <button class="lp-btn" onclick={() => (reOpen = false)}>{$_('common.close')}</button>
    {/snippet}
  </Modal>

  <Modal
    open={moveOpen}
    title={$_('probe.move_title', { values: { name: probe.name } })}
    onclose={() => (moveOpen = false)}
  >
    <label class="lp-field">
      {$_('probe.move_label')}
      <select bind:value={moveTo}>
        {#each $fleet.sites as s (s.site_id)}
          <option value={s.site_id}>{s.name}</option>
        {/each}
      </select>
    </label>
    <p>{$_('probe.move_hint')}</p>
    {#if moveError}<p class="err" role="alert">{moveError}</p>{/if}
    {#snippet footer()}
      <button class="lp-btn" onclick={() => (moveOpen = false)}>{$_('common.cancel')}</button>
      <button class="lp-btn primary" onclick={submitMove} disabled={moveBusy}>
        {moveBusy ? $_('probe.moving') : $_('probe.move_submit')}
      </button>
    {/snippet}
  </Modal>

  <Modal
    open={revokeOpen}
    title={$_('revoke.title', { values: { name: probe.name } })}
    onclose={() => (revokeOpen = false)}
  >
    <!--
      La phrase qui compte est la première et elle est en évidence : une
      confirmation qui laisse planer un doute sur l'historique fait renoncer au
      ménage, et le parc se remplit de sondes mortes.
    -->
    <p class="keep"><strong>{$_('revoke.keeps_data')}</strong></p>
    <p>{$_('revoke.body')}</p>
    <p class="muted">{$_('revoke.rejoin')}</p>
    {#if revokeError}<p class="err" role="alert">{revokeError}</p>{/if}
    {#snippet footer()}
      <button class="lp-btn" onclick={() => (revokeOpen = false)}>{$_('common.cancel')}</button>
      <button class="lp-btn danger" onclick={submitRevoke} disabled={revokeBusy}>
        {revokeBusy ? $_('revoke.working') : $_('revoke.confirm')}
      </button>
    {/snippet}
  </Modal>
{/if}

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
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 14px;
    flex-wrap: wrap;
    margin-bottom: 14px;
  }
  .ident {
    min-width: 0;
    flex: 1 1 260px;
  }
  .title-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  h1 {
    font-size: 21px;
    font-weight: 800;
    letter-spacing: -0.02em;
    margin: 0;
    overflow-wrap: anywhere;
  }
  .edit {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .edit input {
    flex: 1 1 200px;
    font-family: var(--ep-font-sans);
    font-size: 15px;
    font-weight: 700;
  }
  .badges {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 7px;
    font-size: 11.5px;
  }
  .site {
    color: var(--ep-text-secondary);
    font-weight: 600;
  }
  .seen {
    color: var(--ep-text-muted);
    font-size: 11px;
  }
  .dot {
    color: var(--ep-text-dim);
  }
  .acts {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }

  .meta {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 6px;
    margin: 0 0 14px;
  }
  .meta div {
    background: var(--ep-bg-secondary);
    border: 1px solid var(--ep-border);
    border-radius: var(--ep-radius-md);
    padding: 9px 11px;
    min-width: 0;
  }
  dt {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--ep-text-dim);
    margin-bottom: 3px;
  }
  dd {
    margin: 0;
    font-size: 12px;
    color: var(--ep-text-primary);
  }
  /* Un UUID ne doit jamais élargir la page : il se coupe, il ne pousse pas. */
  dd.id {
    font-size: 10.5px;
    color: var(--ep-text-secondary);
    overflow-wrap: anywhere;
  }

  .buffer-alert {
    padding: 11px 13px;
    margin-bottom: 14px;
    border: 1px solid color-mix(in srgb, var(--ep-warning) 50%, var(--ep-border));
    border-left-width: 3px;
    border-radius: var(--ep-radius-md);
    background: color-mix(in srgb, var(--ep-warning) 8%, transparent);
  }
  .buffer-alert strong {
    font-size: 12.5px;
  }
  .buffer-alert p {
    font-size: 11.5px;
    color: var(--ep-text-secondary);
    margin: 4px 0 0;
    line-height: 1.55;
    max-width: 82ch;
  }

  .rangebar {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 12px;
    flex-wrap: wrap;
  }
  .chips {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .chip {
    padding: 5px 11px;
    border-radius: 999px;
    border: 1px solid var(--ep-border);
    background: var(--ep-bg-secondary);
    color: var(--ep-text-secondary);
    font-family: var(--ep-font-sans);
    font-size: 11.5px;
    cursor: pointer;
  }
  .chip.on {
    border-color: var(--ep-accent);
    background: var(--ep-accent-dim);
    color: var(--ep-accent-bright);
    font-weight: 600;
  }

  .charts {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  /* ── Contenu du menu d'actions ─────────────────────────────────────────────
     Chaque item porte son libellé ET sa conséquence : le texte n'a pas été
     raccourci en passant dans le menu, il a été rapproché du geste. Les trois
     familles ne se ressemblent pas — neutre, accent, rouge — pour qu'un clic
     mal visé ne coupe pas une sonde. */
  .mgroup {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--ep-text-dim);
    margin: 8px 8px 4px;
  }
  .mgroup:first-child {
    margin-top: 4px;
  }
  .mintro {
    font-size: 11px;
    line-height: 1.55;
    color: var(--ep-text-muted);
    margin: 0 8px 6px;
  }
  .mi {
    display: flex;
    flex-direction: column;
    gap: 3px;
    width: 100%;
    text-align: left;
    padding: 8px 10px;
    border: 1px solid transparent;
    border-left: 2px solid var(--ep-border-strong);
    border-radius: var(--ep-radius-md);
    background: transparent;
    font-family: var(--ep-font-sans);
    cursor: pointer;
  }
  .mi + .mi {
    margin-top: 2px;
  }
  .mi:hover:not(:disabled) {
    background: var(--ep-glass-bg-md);
  }
  .mi:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
  .mi-label {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--ep-text-primary);
  }
  .mi-hint {
    font-size: 11px;
    line-height: 1.5;
    color: var(--ep-text-muted);
  }
  .mi.accent {
    border-left-color: var(--ep-accent);
  }
  .mi.accent .mi-label {
    color: var(--ep-accent-bright);
  }
  .mi.danger {
    border-left-color: var(--ep-danger);
  }
  .mi.danger .mi-label {
    color: var(--ep-danger);
  }
  /* Les deux gestes qui coupent sont séparés par un trait : le pouce ne glisse
     pas d'une rotation de clé à une révocation. */
  .mdanger {
    margin-top: 8px;
    padding-top: 8px;
    border-top: 1px solid color-mix(in srgb, var(--ep-danger) 35%, var(--ep-border));
  }

  .triage {
    border: 1px solid color-mix(in srgb, var(--ep-danger) 40%, var(--ep-border));
    border-radius: var(--ep-radius-md);
    padding: 11px 13px;
  }
  .triage h3 {
    font-size: 12.5px;
    font-weight: 700;
    margin: 0 0 6px;
    color: var(--ep-danger);
  }
  .triage ul {
    margin: 0;
    padding-left: 18px;
    font-size: 11.5px;
    line-height: 1.6;
    color: var(--ep-text-secondary);
  }
  .triage li + li {
    margin-top: 4px;
  }

  .pending {
    margin-bottom: 14px;
    border: 1px solid color-mix(in srgb, var(--ep-info) 50%, var(--ep-border));
    border-left-width: 3px;
    border-radius: var(--ep-radius-md);
    background: color-mix(in srgb, var(--ep-info) 8%, transparent);
    padding: 11px 13px;
  }
  .pending strong {
    font-size: 12.5px;
    color: var(--ep-text-primary);
  }
  .pending p {
    font-size: 11.5px;
    color: var(--ep-text-secondary);
    margin: 4px 0 0;
    line-height: 1.6;
    max-width: 82ch;
  }
  .done {
    font-size: 12px;
    color: var(--ep-success);
    margin: 0 0 14px;
  }

  .err {
    font-size: 12px;
    color: var(--ep-danger);
    margin: 6px 0 0;
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
  }
  .err.top {
    margin: 0 0 14px;
  }
  /* ── Onglets ──────────────────────────────────────────────────────────── */
  .tabs {
    display: flex;
    gap: 2px;
    margin: 18px 0 14px;
    border-bottom: 1px solid var(--ep-border);
    overflow-x: auto;
  }
  .tab {
    padding: 8px 14px;
    border: none;
    border-bottom: 2px solid transparent;
    background: none;
    color: var(--ep-text-secondary);
    font: inherit;
    font-size: 13px;
    white-space: nowrap;
    cursor: pointer;
  }
  .tab:hover {
    color: var(--ep-text-primary);
  }
  .tab.on {
    color: var(--ep-text-primary);
    border-bottom-color: var(--ep-accent);
  }
  .tab:focus-visible {
    outline: 2px solid var(--ep-accent);
    outline-offset: -2px;
  }

  .panel {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .block {
    padding: 16px 18px;
  }
  .bh {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-start;
    justify-content: space-between;
    gap: 10px;
    margin-bottom: 12px;
  }
  .bsub {
    margin: 3px 0 0;
    font-size: 12px;
    color: var(--ep-text-dim);
  }
  .empty {
    margin: 0;
    font-size: 13px;
    color: var(--ep-text-dim);
  }
  .scanmeta {
    margin: 0 0 10px;
    font-size: 11.5px;
    color: var(--ep-text-dim);
  }

  .targets {
    list-style: none;
    margin: 0 0 12px;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .targets li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 6px 8px;
    border-radius: 5px;
    background: var(--ep-bg-tertiary);
    font-size: 13px;
  }
  .addrow,
  .inline {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .addrow .lp-input,
  .inline .lp-input {
    max-width: 240px;
  }

  /* Un tableau large défile dans sa boîte : la page, elle, ne défile jamais
     horizontalement. */
  .tablewrap {
    overflow-x: auto;
  }
  .grid {
    width: 100%;
    border-collapse: collapse;
    font-size: 12.5px;
  }
  .grid th {
    text-align: left;
    padding: 6px 10px 6px 0;
    border-bottom: 1px solid var(--ep-border);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--ep-text-dim);
    font-weight: 500;
    white-space: nowrap;
  }
  .grid td {
    padding: 6px 10px 6px 0;
    border-bottom: 1px solid var(--ep-border);
    vertical-align: top;
  }
  .grid .num {
    text-align: right;
    padding-right: 14px;
  }
  .grid .srv {
    max-width: 240px;
    overflow-wrap: anywhere;
  }

  .cstate {
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 3px;
    border: 1px solid var(--ep-border);
    white-space: nowrap;
  }
  .cstate.done {
    color: var(--ep-success);
    border-color: var(--ep-success);
  }
  .cstate.failed {
    color: var(--ep-danger);
    border-color: var(--ep-danger);
  }
  .cnote {
    display: block;
    margin-top: 2px;
    font-size: 11px;
    color: var(--ep-text-dim);
    overflow-wrap: anywhere;
  }

  .engine {
    font-size: 10px;
    letter-spacing: 0.4px;
    color: var(--ep-text-dim);
    white-space: nowrap;
  }

  .keep {
    color: var(--ep-text-primary);
    border-left: 2px solid var(--ep-success);
    padding-left: 10px;
  }
  .muted {
    color: var(--ep-text-muted);
    font-size: 12px;
  }
</style>
