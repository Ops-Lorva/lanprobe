<script lang="ts">
  import { onMount } from 'svelte';
  import { pushNow, restoreFromHub } from '../hubConfigBackup';
  import { invoke } from '@tauri-apps/api/core';
  import { _ } from 'svelte-i18n';
  import { settings, type Theme, type Layout, type Lang, type SpeedtestEngine, type Palette } from '../stores/settings';
  import { portscanProfiles, type PortScanProfile } from '../stores/portscanProfiles';
  import { profiles, type Profile } from '../stores/profiles';
  import { get } from 'svelte/store';
  import { selectedInterface } from '../stores/selectedInterface';
  import { scheduler } from '../stores/scheduler';
  import ScreenHead from './ScreenHead.svelte';
  import StateMark from './StateMark.svelte';
  import SchedulerControl from './SchedulerControl.svelte';
  import { hubStatus } from '../stores/hub';
  import { hubMark } from '../status';
  import { screenKind } from '../screen';
  import { locale } from 'svelte-i18n';

  // Barre latérale : les réglages en onglets. Ailleurs (onglet Config,
  // surimpression de la page unique), en panneaux côte à côte.
  const kind = screenKind();
  const tabbed = kind === 'page';
  let tab = $state<'general' | 'hub' | 'scan' | 'about'>('general');
  const lang = $derived($locale ?? 'en');
  let codeInput = $state<HTMLInputElement | null>(null);
  function retype() {
    hubError = '';
    hubCode = '';
    codeInput?.focus();
  }

  // Les profils de scan tels que l'écran les montre : une sonde rattachée à un
  // hub n'affiche plus ses profils de base (contrat § 25).
  const visiblePortscanProfiles = portscanProfiles.visible;

  // ── Rattachement à un hub ───────────────────────────────────────────────
  let hub = $state<{ enrolled: boolean; url: string; name: string; site: string }>({
    enrolled: false, url: '', name: '', site: '',
  });
  let hubUrl = $state('');
  /** Nombre de réglages rendus par le hub au dernier enrôlement. */
  let hubRestored = $state(0);

  // ── Adresse du hub : schéma séparé de l'hôte ─────────────────────────────
  //
  // Taper « https:// » au clavier est pénible et se rate — et une adresse
  // fausse donne un enrôlement qui échoue sans dire pourquoi. Le schéma est
  // donc un bouton à deux positions, l'hôte un champ de texte simple.
  let scheme = $state<'https://' | 'http://'>('https://');
  let hubHost = $state('');

  function toggleScheme() {
    scheme = scheme === 'https://' ? 'http://' : 'https://';
  }

  // Coller une URL complète dans le champ d'hôte est le geste naturel : on
  // en extrait le schéma plutôt que de produire « https://https://… ».
  $effect(() => {
    const match = /^(https?:\/\/)(.*)$/i.exec(hubHost);
    if (match) {
      scheme = match[1].toLowerCase() as 'https://' | 'http://';
      hubHost = match[2];
    }
  });

  $effect(() => {
    hubUrl = hubHost.trim() ? `${scheme}${hubHost.trim()}` : '';
  });
  let hubName = $state('');
  let hubCode = $state('');

  /**
   * Met le code en forme pendant la frappe : majuscules et tiret posé tout
   * seul. Le code est dicté au téléphone — on le tape donc souvent en
   * minuscules, ou sans le tiret. Le refuser pour ça serait absurde quand la
   * correction est mécanique.
   */
  function formatCode(raw: string): string {
    const clean = raw.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 8);
    return clean.length > 4 ? `${clean.slice(0, 4)}-${clean.slice(4)}` : clean;
  }
  /**
   * ⚠️ Toujours faux, et plus proposé nulle part.
   *
   * Le champ reste envoyé parce que le hub et les sondes déjà déployées le
   * connaissent : une sonde qui le porte dans sa configuration doit continuer
   * de fonctionner à la lettre après la mise à jour. Ce qui disparaît, c'est
   * la possibilité de l'activer — remplacée par l'épinglage, qui dit « ce
   * hub-là » au lieu de « je ne vérifie plus rien ».
   */
  const hubSelfSigned = false;
  let hubBusy = $state(false);
  let hubError = $state('');

  /**
   * Épinglage du certificat, à la première connexion.
   *
   * ⚠️ Remplace la case « accepter un certificat auto-signé », qui acceptait
   * **n'importe quel** certificat : n'importe qui sur le chemin pouvait se
   * faire passer pour le hub et repartir avec le jeton de la sonde. Cocher
   * cette case ne disait pas « je fais confiance à MON hub », elle disait
   * « je ne vérifie plus rien ».
   */
  let pendingPin = $state<{ host: string; fingerprint: string } | null>(null);

  async function refreshHub() {
    try {
      hub = await invoke('cmd_hub_status');
    } catch {
      // Le rattachement est optionnel : son indisponibilité ne doit pas
      // empêcher le reste des réglages de s'afficher.
    }
    // La barre latérale et le tableau de bord lisent le même état.
    void hubStatus.refresh();
  }
  onMount(() => hubStatus.watch());
  const live = $derived($hubStatus);
  const liveMark = $derived(hubMark(live));
  const sinceMin = $derived(live.last_beat_at ? Math.max(1, Math.round((Date.now() / 1000 - live.last_beat_at) / 60)) : null);
  const beatAt = $derived(live.last_beat_at ? new Date(live.last_beat_at * 1000).toLocaleTimeString(lang) : null);

  async function connectHub(pin: string = '') {
    hubError = '';
    hubBusy = true;
    try {
      const result = (await invoke('cmd_hub_enroll', {
        args: {
          url: hubUrl.trim(),
          // Facultatif : vide, c'est le hub qui nomme. Et un ré-enrôlement
          // ignore ce champ de toute façon — il répare une sonde existante.
          name: hubName.trim() || null,
          code: hubCode.trim() || null,
          allow_self_signed: hubSelfSigned,
          pin: pin || null,
        },
      })) as { needs_pin?: boolean; host?: string; fingerprint?: string };

      // Ce n'est pas un échec : c'est le moment où l'utilisateur doit
      // décider. Le rattachement reprendra avec l'empreinte confirmée.
      if (result?.needs_pin) {
        pendingPin = { host: result.host ?? '', fingerprint: result.fingerprint ?? '' };
        return;
      }
      pendingPin = null;
      hubCode = '';
      await refreshHub();

      // Le hub gardait peut-être les profils de cette machine : c'est le seul
      // moment où une machine réinstallée peut les retrouver. Ne remplace que
      // ce qui manque en local — voir `hubConfigBackup`.
      const restored = await restoreFromHub();
      if (restored > 0) {
        hubRestored = restored;
        await profiles.init();
        await portscanProfiles.init();
      } else {
        // Rien à restaurer : on dépose ce que cette machine a, pour la
        // prochaine fois. C'est l'enrôlement initial d'une sonde qui a déjà
        // servi en local.
        void pushNow();
      }
    } catch (e) {
      hubError = String(e);
    } finally {
      hubBusy = false;
    }
  }

  async function forgetHub() {
    try {
      await invoke('cmd_hub_forget');
      await refreshHub();
    } catch (e) {
      hubError = String(e);
    }
  }

  const langs: { value: Lang; labelKey: string }[] = [
    { value: 'en', labelKey: 'settings.language.english' },
    { value: 'fr', labelKey: 'settings.language.french' },
    { value: 'es', labelKey: 'settings.language.spanish' },
  ];

  // isWeb = client d'un desktop en mode serveur → read-only (le desktop pilote).
  // isHeadless = client du binaire headless standalone → full control.
  const isWeb = typeof window !== 'undefined' && (window as any).__LANPROBE_WEB__ === true;
  const isHeadless = typeof window !== 'undefined' && (window as any).__LANPROBE_HEADLESS__ === true;
  // En mode headless, le client web a tous les droits : il n'y a pas de desktop
  // pour piloter la config. En mode web-du-desktop, tout reste read-only.
  const readOnly = isWeb && !isHeadless;

  let appVersion = $state('…');
  let installType = $state<'pkg' | 'dmg' | 'unknown' | 'headless-server'>('unknown');
  let serverPlatform = $state<'windows' | 'macos' | 'linux' | null>(null);

  onMount(refreshHub);

  onMount(async () => {
    try { appVersion = await invoke<string>('cmd_app_version'); } catch {}
    try { installType = await invoke<'pkg' | 'dmg' | 'unknown' | 'headless-server'>('cmd_install_type'); } catch {}
    if (isHeadless) {
      try { serverPlatform = await invoke<'windows' | 'macos' | 'linux'>('cmd_get_platform'); } catch {}
    }
    await portscanProfiles.init();
    await scheduler.init();
  });

  const themes: { value: Theme; labelKey: string; descKey: string }[] = [
    { value: 'system', labelKey: 'settings.theme.system', descKey: 'settings.theme.system_desc' },
    { value: 'dark',   labelKey: 'settings.theme.dark',   descKey: 'settings.theme.dark_desc' },
    { value: 'light',  labelKey: 'settings.theme.light',  descKey: 'settings.theme.light_desc' },
  ];

  const palettes: { value: Palette; label: string; dark: string; light: string }[] = [
    { value: 'indigo',  label: 'Indigo',  dark: '#6366f1', light: '#4f46e5' },
    { value: 'cyan',    label: 'Cyan',    dark: '#06b6d4', light: '#0891b2' },
    { value: 'emerald', label: 'Emerald', dark: '#10b981', light: '#059669' },
    { value: 'rose',    label: 'Rose',    dark: '#f43f5e', light: '#e11d48' },
    { value: 'amber',   label: 'Amber',   dark: '#f59e0b', light: '#d97706' },
    { value: 'slate',   label: 'Slate',   dark: '#64748b', light: '#475569' },
  ];

  const layouts: { value: Layout; labelKey: string; descKey: string }[] = [
    { value: 'sidebar', labelKey: 'settings.layout.sidebar', descKey: 'settings.layout.sidebar_desc' },
    { value: 'single',  labelKey: 'settings.layout.single',  descKey: 'settings.layout.single_desc' },
    { value: 'grouped', labelKey: 'settings.layout.grouped', descKey: 'settings.layout.grouped_desc' },
  ];

  // Export / Import ------------------------------------------------------
  // Tout est géré côté frontend via Blob download + <input type="file">.
  // L'export ne contient que les profils réseau et de ports personnalisés :
  // l'utilisateur ne veut pas transporter le choix d'interface (spécifique
  // à la machine) ni les presets de ports built-in (régénérés à chaque
  // version de LanProbe).
  interface ConfigBundle {
    _meta: { app: 'lanprobe'; version: number; exported_at: string };
    profiles: Profile[];
    portscan_profiles: PortScanProfile[];
  }
  let importInput = $state<HTMLInputElement | null>(null);
  let importStatus = $state('');
  let importTimer: ReturnType<typeof setTimeout> | null = null;
  function flashImport(msg: string) {
    importStatus = msg;
    if (importTimer) clearTimeout(importTimer);
    importTimer = setTimeout(() => { importStatus = ''; importTimer = null; }, 5000);
  }

  function exportConfig() {
    const customPortscan = get(portscanProfiles).filter(p => !p.builtin && !p.id.startsWith('builtin:'));
    const bundle: ConfigBundle = {
      _meta: { app: 'lanprobe', version: 1, exported_at: new Date().toISOString() },
      profiles: get(profiles),
      portscan_profiles: customPortscan,
    };
    const blob = new Blob([JSON.stringify(bundle, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    const stamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
    a.href = url;
    a.download = `lanprobe-config-${stamp}.json`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
    flashImport($_('settings.export_import.exported', { values: { profiles: bundle.profiles.length, portscan: bundle.portscan_profiles.length } }));
  }

  async function onImportFile(ev: Event) {
    const input = ev.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    try {
      const text = await file.text();
      const parsed = JSON.parse(text) as ConfigBundle;
      if (parsed?._meta?.app !== 'lanprobe') {
        throw new Error('Not a LanProbe config');
      }
      // Merge: on ajoute ce qui n'existe pas (par id), on saute le reste.
      const existingProfileIds = new Set(get(profiles).map(p => p.id));
      let addedProfiles = 0;
      for (const p of parsed.profiles ?? []) {
        if (p && p.id && !existingProfileIds.has(p.id)) {
          profiles.add(p);
          addedProfiles++;
        }
      }
      const existingPortscanIds = new Set(get(portscanProfiles).map(p => p.id));
      let addedPortscan = 0;
      for (const p of parsed.portscan_profiles ?? []) {
        if (p && p.id && !p.builtin && !p.id.startsWith('builtin:') && !existingPortscanIds.has(p.id)) {
          portscanProfiles.add(p);
          addedPortscan++;
        }
      }
      flashImport($_('settings.export_import.imported', { values: { profiles: addedProfiles, portscan: addedPortscan } }));
    } catch (e) {
      flashImport(`${$_('common.error')}: ${e}`);
    } finally {
      input.value = '';
    }
  }

  const refreshOptions: { value: number; labelKey: string }[] = [
    { value: 0,  labelKey: 'settings.dashboard_refresh.disabled' },
    { value: 2,  labelKey: 'settings.dashboard_refresh.every_2s' },
    { value: 5,  labelKey: 'settings.dashboard_refresh.every_5s' },
    { value: 10, labelKey: 'settings.dashboard_refresh.every_10s' },
    { value: 30, labelKey: 'settings.dashboard_refresh.every_30s' },
  ];
</script>

{#snippet general()}
  <div class="setrow">
    <div><div class="k">{$_('settings.layout.title')}</div></div>
    <div class="seg" role="group" aria-label={$_('settings.layout.title')}>
      {#each layouts as l}
        <button type="button" aria-pressed={$settings.layout === l.value} disabled={readOnly}
                title={$_(l.descKey)} onclick={() => settings.setLayout(l.value)}>{$_(l.labelKey)}</button>
      {/each}
    </div>
  </div>
  <div class="setrow">
    <div><div class="k">{$_('settings.theme.title')}</div></div>
    <div class="seg" role="group" aria-label={$_('settings.theme.title')}>
      {#each themes as t}
        <button type="button" aria-pressed={$settings.theme === t.value} disabled={readOnly}
                title={$_(t.descKey)} onclick={() => settings.setTheme(t.value)}>{$_(t.labelKey)}</button>
      {/each}
    </div>
  </div>
  <div class="setrow">
    <div><div class="k">{$_('settings.palette.title')}</div><div class="d">{$_('settings.palette.hint')}</div></div>
    <div class="swatches" role="group" aria-label={$_('settings.palette.title')}>
      {#each palettes as p}
        <button type="button" class="swatch" aria-pressed={$settings.palette === p.value} disabled={readOnly}
                style="--swatch-dark: {p.dark}; --swatch-light: {p.light}" title={$_(`settings.palette.${p.value}`)}
                onclick={() => settings.setPalette(p.value)}>
          <span class="dot" aria-hidden="true"></span><span class="visually-hidden">{$_(`settings.palette.${p.value}`)}</span>
        </button>
      {/each}
    </div>
  </div>
  <div class="setrow">
    <div><label class="k" for="set-lang">{$_('settings.language.title')}</label></div>
    <select id="set-lang" class="select" value={$settings.lang} disabled={readOnly}
            onchange={(e) => settings.setLanguage((e.currentTarget as HTMLSelectElement).value as Lang)}>
      {#each langs as l}<option value={l.value}>{$_(l.labelKey)}</option>{/each}
    </select>
  </div>
  <div class="setrow">
    <div><label class="k" for="set-refresh">{$_('settings.dashboard_refresh.label')}</label><div class="d">{$_('settings.dashboard_refresh.hint')}</div></div>
    <select id="set-refresh" class="select" value={$settings.dashboardRefreshSec} disabled={readOnly}
            onchange={(e) => settings.setDashboardRefresh(Number((e.target as HTMLSelectElement).value))}>
      {#each refreshOptions as opt}<option value={opt.value}>{$_(opt.labelKey)}</option>{/each}
    </select>
  </div>
{/snippet}

{#snippet schedulerRows()}
  <div class="setrow">
    <div><div class="k">{$_('nav.discovery')}</div></div>
    <SchedulerControl field="discovery_interval_min" />
  </div>
  <div class="setrow">
    <div><div class="k">{$_('nav.speedtest')}</div></div>
    <SchedulerControl field="speedtest_interval_min" />
  </div>
  <div class="setrow">
    <div><div class="k">{$_('settings.scheduler.portscan')}</div></div>
    <SchedulerControl field="portscan_interval_min" />
  </div>
  <div class="setrow">
    <div><label class="k" for="set-autops">{$_('settings.auto_port_scan.label')}</label><div class="d">{$_('settings.auto_port_scan.hint')}</div></div>
    <select id="set-autops" class="select" value={$settings.autoPortScanProfileId ?? ''} disabled={readOnly}
      onchange={(e) => {
        const v = (e.target as HTMLSelectElement).value;
        settings.setAutoPortScanProfile(v === '' ? null : v);
      }}>
      <option value="">{$_('settings.auto_port_scan.disabled')}</option>
      <!-- ⚠️ Les profils AFFICHÉS, pas tous : une sonde rattachée à un hub
           n'affiche plus ses profils de base (§ 25). Proposer ici un
           profil invisible ailleurs ferait tourner un scan automatique
           avec une liste de ports introuvable à l'écran. -->
      {#each $visiblePortscanProfiles as p (p.id)}
        <option value={p.id}>{p.from_hub ? `★ ${p.name}` : p.name} ({p.tcp_ports.length} TCP, {p.udp_ports.length} UDP)</option>
      {/each}
    </select>
  </div>
{/snippet}

{#snippet speedtest()}
  <div class="setrow">
    <div><div class="k">{$_('settings.speedtest.engine')}</div>
      <div class="d">{$settings.speedtestEngine === 'iperf3' ? $_('settings.speedtest.iperf3_desc') : $_('settings.speedtest.ookla_desc')}</div></div>
    <div class="seg" role="group" aria-label={$_('settings.speedtest.engine')}>
      <button type="button" aria-pressed={$settings.speedtestEngine === 'ookla'} onclick={() => settings.setSpeedtestEngine('ookla')}>{$_('settings.speedtest.ookla')}</button>
      <button type="button" aria-pressed={$settings.speedtestEngine === 'iperf3'} onclick={() => settings.setSpeedtestEngine('iperf3')}>{$_('settings.speedtest.iperf3')}</button>
    </div>
  </div>
  {#if $settings.speedtestEngine === 'iperf3'}
    <div class="setrow">
      <div><label class="k" for="set-iperf">{$_('settings.speedtest.server_label')}</label><div class="d">{$_('settings.speedtest.iperf_hint')}</div></div>
      <input id="set-iperf" class="input m" type="text" placeholder="iperf.example.com" value={$settings.iperfServer}
             oninput={(e) => settings.setIperfServer((e.target as HTMLInputElement).value)} />
    </div>
  {/if}
{/snippet}

{#snippet hubForm()}
  <!-- Rattachement à un hub : entièrement optionnel. Sans lui, LanProbe
       fonctionne exactement comme avant, sans compte ni serveur. -->
  {#if hub.enrolled}
    <div class="hubstate">
      <StateMark mark={liveMark}
        label={live.state === 'ok' && hub.site ? $_('settings.hub.attached_to', { values: { site: hub.site } }) : undefined} />
      {#if live.state === 'ok' && beatAt}<span class="muted">{$_('settings.hub.last_beat', { values: { at: beatAt } })}</span>{/if}
      {#if live.state === 'degraded' && sinceMin}<span class="muted">{$_('settings.hub.since_min', { values: { n: sinceMin } })}</span>{/if}
      {#if live.buffered_points > 0}<span class="muted">{$_('dashboard.hub_buffered', { values: { n: live.buffered_points } })}</span>{/if}
    </div>
    {#if live.last_error && live.state !== 'ok'}<p class="hint m">{live.last_error}</p>{/if}
    <dl class="kv" style="margin-top:12px">
      <dt>{$_('settings.hub.url')}</dt><dd class="m">{hub.url}</dd>
      <dt>{$_('settings.hub.name')}</dt><dd>{hub.name || '—'}</dd>
      {#if hub.site}<dt>{$_('settings.hub.site')}</dt><dd>{hub.site}</dd>{/if}
    </dl>
    {#if hubRestored > 0}
      <!-- Le dire : quelqu'un qui vient de réinstaller une machine ne
           s'attend pas à retrouver ses profils, et les découvrirait par
           hasard. -->
      <div class="banner ok" style="margin-top:12px"><span class="grow">{$_('settings.hub.restored', { values: { n: hubRestored } })}</span></div>
    {/if}
    <div class="setrow" style="border:0">
      <div class="d">{$_('settings.hub.forget_hint')}</div>
      <div><button class="btn danger" type="button" onclick={forgetHub} disabled={readOnly}>{$_('settings.hub.forget')}</button></div>
    </div>
  {:else}
    <p class="hint" style="margin-bottom:12px">{$_('settings.hub.lead')}</p>
    <form class="hubform" onsubmit={(e) => { e.preventDefault(); if (!pendingPin) connectHub(); }}>
      <div class="field">
        <label for="hub-host">{$_('settings.hub.url')}</label>
        <div class="input-row" style="flex-wrap:nowrap">
          <!-- Le schéma est un bouton, pas un caractère à taper : c'est la
               partie de l'adresse qui ne varie qu'entre deux valeurs, et
               c'est celle qu'on tape le plus mal. -->
          <button type="button" class="btn m scheme" onclick={toggleScheme} disabled={readOnly}
                  title={$_('settings.hub.scheme_hint')}>{scheme}</button>
          <input id="hub-host" class="input m" type="text" bind:value={hubHost} placeholder="hub.exemple.fr"
                 autocapitalize="off" autocorrect="off" spellcheck="false" disabled={readOnly} />
        </div>
      </div>
      <div class="field">
        <label for="hub-code">{$_('settings.hub.code')}</label>
        <input id="hub-code" bind:this={codeInput} class="input m code" type="text" value={hubCode}
               oninput={(e) => (hubCode = formatCode(e.currentTarget.value))}
               placeholder="K7M2-4PQX" spellcheck="false" autocapitalize="characters" autocomplete="off"
               aria-invalid={hubError ? 'true' : undefined} disabled={readOnly} />
        <span class="hint">{$_('settings.hub.code_hint')}</span>
      </div>
      <div class="field">
        <label for="hub-name">{$_('settings.hub.name')}</label>
        <input id="hub-name" class="input" type="text" bind:value={hubName} placeholder={$_('settings.hub.name_ph')} disabled={readOnly} />
        <span class="hint">{$_('settings.hub.name_hint')}</span>
      </div>
      {#if pendingPin}
        <!--
          L'empreinte se compare à l'œil, donc elle s'affiche en entier et en
          chasse fixe. La tronquer ferait comparer les premiers caractères,
          ce qui ne prouve rien.
        -->
        <div class="banner">
          <span class="grow"><b>{$_('settings.hub.pin_title', { values: { host: pendingPin.host } })}</b></span>
          <code class="fp">{pendingPin.fingerprint}</code>
          <span class="hint">{$_('settings.hub.pin_hint')}</span>
          <span class="input-row">
            <button class="btn" type="button" onclick={() => (pendingPin = null)} disabled={hubBusy}>{$_('common.cancel')}</button>
            <button class="btn primary" type="button" onclick={() => connectHub(pendingPin?.fingerprint ?? '')} disabled={hubBusy}>{$_('settings.hub.pin_confirm')}</button>
          </span>
        </div>
      {:else}
        <div>
          <button class="btn primary" type="submit" disabled={readOnly || hubBusy || !hubUrl || !hubCode} aria-busy={hubBusy}>
            {hubBusy ? $_('settings.hub.connecting') : $_('settings.hub.connect')}
          </button>
        </div>
      {/if}
    </form>
  {/if}
{/snippet}

{#snippet hubState()}
  <!-- Les quatre états de l'enrôlement, dans l'ordre où on les rencontre. -->
  {#if hubError}
    <div class="hubstate" role="alert">
      <StateMark mark={{ tone: 'bad', shape: 'square', key: 'settings.hub.error' }} />
      <button class="btn sm" type="button" onclick={retype}>{$_('settings.hub.retype')}</button>
    </div>
    <p class="hint m">{hubError}</p>
  {:else if hub.enrolled}
    <div class="hubstate"><StateMark mark={liveMark} /></div>
  {:else if hubBusy}
    <div class="hubstate"><StateMark mark={{ tone: 'none', shape: 'ring', key: 'settings.hub.connecting' }} /></div>
  {:else}
    <div class="hubstate"><StateMark mark={liveMark} /></div>
  {/if}
{/snippet}

{#snippet config()}
  <div class="setrow">
    <div>
      <div class="k">{$_('settings.export_import.summary', { values: { profiles: $profiles.length, portscan: $portscanProfiles.filter(p => !p.builtin && !p.id.startsWith('builtin:')).length } })}</div>
      <div class="d">{$_('settings.export_import.hint')}</div>
    </div>
    <div class="input-row">
      <button class="btn" type="button" onclick={exportConfig} disabled={readOnly}>{$_('settings.export_import.export')}</button>
      <button class="btn" type="button" onclick={() => importInput?.click()} disabled={readOnly}>{$_('settings.export_import.import')}</button>
      <input bind:this={importInput} type="file" accept=".json,application/json" onchange={onImportFile} hidden />
    </div>
  </div>
  {#if importStatus}<div class="banner info" role="status"><span class="grow">{importStatus}</span></div>{/if}
  <div class="setrow">
    <div><div class="k">{$_('settings.about.version')}</div></div>
    <div class="num">
      LanProbe {appVersion},
      {#if isHeadless && serverPlatform}
        {serverPlatform === 'windows' ? 'Windows' : serverPlatform === 'macos' ? 'macOS' : 'Linux'} <span class="muted">({$_('settings.about.server')})</span>
      {:else if navigator.userAgent.includes('Win')}Windows
      {:else if navigator.userAgent.includes('Mac')}macOS
      {:else}Linux{/if}
    </div>
  </div>
  {#if isHeadless}
    <div class="setrow">
      <div><div class="k">{$_('settings.about.install_type')}</div></div>
      <div><span class="tag">{$_('settings.about.headless')}</span></div>
    </div>
  {:else if navigator.userAgent.includes('Mac') && (installType === 'pkg' || installType === 'dmg')}
    <div class="setrow">
      <div><div class="k">{$_('settings.about.install_type')}</div></div>
      <div><span class="tag">{installType === 'pkg' ? 'PKG' : 'DMG'}</span> <span class="muted">{$_(installType === 'pkg' ? 'settings.about.install_hint_pkg' : 'settings.about.install_hint_dmg')}</span></div>
    </div>
  {/if}
{/snippet}

<div class="screen">
  <ScreenHead title={$_('settings.title')} />
  <div class="screen-body">
    {#if readOnly}<div class="banner info"><span class="grow">{$_('settings.web_readonly_hint')}</span></div>{/if}

    {#if tabbed}
      <div class="tabs" role="tablist" aria-label={$_('settings.title')}>
        {#each [['general', 'settings.tabs.general'], ['hub', 'settings.tabs.hub'], ['scan', 'settings.tabs.scan'], ['about', 'settings.tabs.about']] as [id, key]}
          <button type="button" role="tab" aria-selected={tab === id} aria-current={tab === id ? 'page' : undefined}
                  onclick={() => (tab = id as typeof tab)}>{$_(key)}</button>
        {/each}
      </div>
      {#if tab === 'general'}
        <section class="panel">{@render general()}</section>
      {:else if tab === 'hub'}
        {#if hub.enrolled}
          <section class="panel" aria-labelledby="h-he" style="max-width:720px">
            <h2 id="h-he">{$_('settings.hub.title')}</h2>
            {@render hubForm()}
          </section>
        {:else}
          <div class="grid2">
            <section class="panel" aria-labelledby="h-he">
              <h2 id="h-he">{$_('settings.hub.enrol_title')}</h2>
              {@render hubForm()}
            </section>
            <section class="panel" aria-labelledby="h-hs">
              <h2 id="h-hs">{$_('settings.hub.state')}</h2>
              {@render hubState()}
            </section>
          </div>
        {/if}
      {:else if tab === 'scan'}
        <div class="grid2">
          <section class="panel" aria-labelledby="h-sc"><h2 id="h-sc">{$_('settings.scheduler.title')}</h2>{@render schedulerRows()}</section>
          <section class="panel" aria-labelledby="h-st"><h2 id="h-st">{$_('settings.speedtest.title')}</h2>{@render speedtest()}</section>
        </div>
      {:else}
        <section class="panel">{@render config()}</section>
      {/if}
    {:else}
      <div class="setgrid">
        <section class="panel" aria-labelledby="h-g"><h2 id="h-g">{$_('settings.tabs.general')}</h2>{@render general()}</section>
        <section class="panel" aria-labelledby="h-sc"><h2 id="h-sc">{$_('settings.scheduler.title')}</h2>{@render schedulerRows()}</section>
        <section class="panel" aria-labelledby="h-st"><h2 id="h-st">{$_('settings.speedtest.title')}</h2>{@render speedtest()}</section>
        <section class="panel" aria-labelledby="h-h">
          <h2 id="h-h">{hub.enrolled ? $_('settings.hub.title') : $_('settings.hub.enrol_title')}</h2>
          {#if !hub.enrolled}{@render hubState()}{/if}
          {@render hubForm()}
        </section>
        <section class="panel" aria-labelledby="h-c"><h2 id="h-c">{$_('settings.tabs.about')}</h2>{@render config()}</section>
      </div>
    {/if}
  </div>
</div>

<style>
  .swatches { display: flex; flex-wrap: wrap; gap: var(--space-xs); }
  .swatch { width: 30px; height: 30px; display: grid; place-items: center; border: 1px solid var(--color-rule-2); border-radius: 50%; background: var(--color-paper-2); cursor: pointer; padding: 0; }
  .swatch .dot { width: 16px; height: 16px; border-radius: 50%; background: var(--swatch-dark); }
  :global([data-theme='light']) .swatch .dot { background: var(--swatch-light); }
  @media (prefers-color-scheme: light) { :global(:root:not([data-theme])) .swatch .dot { background: var(--swatch-light); } }
  .swatch[aria-pressed='true'] { border-color: var(--color-ink); box-shadow: 0 0 0 1px var(--color-ink); }
  .swatch[disabled] { cursor: not-allowed; opacity: 0.6; }
  .hubform { display: grid; gap: var(--space-md); }
  .code { font-size: 1.25rem; letter-spacing: 0.12em; height: 44px; max-width: 220px; }
  .scheme { flex: none; min-width: 84px; padding: 0 var(--space-sm); }
  .hubstate { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-xs) var(--space-md); margin: var(--space-2xs) 0 var(--space-xs); }
  .fp { flex-basis: 100%; overflow-wrap: anywhere; font-size: var(--text-sm); }
  .panel > h2 { margin-bottom: var(--space-xs); }
</style>
