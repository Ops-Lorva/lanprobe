<script lang="ts">
  import { onMount } from 'svelte';
  import { _ } from 'svelte-i18n';
  import { portscan, type ScanEntry } from '../stores/portscan';
  import { portscanProfiles, type PortScanProfile } from '../stores/portscanProfiles';
  import { scheduler } from '../stores/scheduler';
  import { get } from 'svelte/store';

  let newIp = $state('');
  let showManager = $state(false);
  let editing = $state<PortScanProfile | null>(null);
  let editName = $state('');
  let editTcpText = $state('');
  let editUdpText = $state('');

  onMount(() => { portscanProfiles.init(); });

  let activeId = $state('builtin:common');
  portscanProfiles.active.subscribe(v => { activeId = v; });
  const activeProfile = $derived($portscanProfiles.find(p => p.id === activeId) ?? $portscanProfiles[0]);

  function parsePorts(text: string): number[] {
    const out = new Set<number>();
    for (const raw of text.split(/[,\s\n]+/)) {
      const t = raw.trim();
      if (!t) continue;
      const range = t.match(/^(\d+)-(\d+)$/);
      if (range) {
        const a = Math.max(1, Math.min(65535, parseInt(range[1], 10)));
        const b = Math.max(1, Math.min(65535, parseInt(range[2], 10)));
        for (let p = Math.min(a, b); p <= Math.max(a, b); p++) out.add(p);
      } else {
        const p = parseInt(t, 10);
        if (p >= 1 && p <= 65535) out.add(p);
      }
    }
    return [...out].sort((a, b) => a - b);
  }

  async function addHost() {
    const ip = newIp.trim();
    if (!ip) return;
    newIp = '';
    await portscan.add(ip, activeProfile?.tcp_ports, activeProfile?.udp_ports, activeProfile?.id ?? null, activeProfile?.name ?? null);
    const sched = get(scheduler);
    scheduler.save({ ...sched, portscan_targets: [...new Set([...sched.portscan_targets, ip])] });
  }

  function removeHost(ip: string) {
    portscan.remove(ip);
    const sched = get(scheduler);
    scheduler.save({ ...sched, portscan_targets: sched.portscan_targets.filter(t => t !== ip) });
  }

  function rescan(entry: ScanEntry) {
    const prof = $portscanProfiles.find(p => p.id === entry.profileId) ?? activeProfile;
    portscan.add(entry.ip, prof?.tcp_ports, prof?.udp_ports, prof?.id ?? null, prof?.name ?? null);
  }

  function rescanWith(ip: string, profileId: string) {
    const prof = $portscanProfiles.find(p => p.id === profileId);
    if (!prof) return;
    portscan.add(ip, prof.tcp_ports, prof.udp_ports, prof.id, prof.name);
  }

  function formatScannedAt(ts: number | null): string {
    if (!ts) return '';
    const delta = Math.floor((Date.now() - ts) / 1000);
    if (delta < 5) return $_('port_scan.just_now');
    if (delta < 60) return `${delta}s`;
    if (delta < 3600) return `${Math.floor(delta / 60)}m`;
    if (delta < 86400) return `${Math.floor(delta / 3600)}h`;
    return `${Math.floor(delta / 86400)}d`;
  }

  function openNew() {
    editing = { id: `custom:${Date.now()}`, name: '', tcp_ports: [], udp_ports: [] };
    editName = '';
    editTcpText = '';
    editUdpText = '';
  }

  function openEdit(p: PortScanProfile) {
    editing = { ...p };
    editName = p.name;
    editTcpText = p.tcp_ports.join(', ');
    editUdpText = p.udp_ports.join(', ');
  }

  function saveEdit() {
    if (!editing || !editName.trim()) return;
    const next: PortScanProfile = {
      id: editing.id,
      name: editName.trim(),
      tcp_ports: parsePorts(editTcpText),
      udp_ports: parsePorts(editUdpText),
    };
    const exists = $portscanProfiles.some(p => p.id === next.id);
    if (exists) portscanProfiles.edit(next);
    else portscanProfiles.add(next);
    editing = null;
  }

  function removeProfile(p: PortScanProfile) {
    if (p.builtin) return;
    portscanProfiles.remove(p.id);
    if (activeId === p.id) portscanProfiles.setActive('builtin:common');
  }

  function openCount(e: ScanEntry): number {
    return e.tcpResults.filter(r => r.open).length + e.udpResults.filter(r => r.open).length;
  }
</script>

<div class="page">
  <div class="header">
    <h1>{$_('port_scan.title')}</h1>
    <div class="add-row">
      <select class="profile-select" value={activeId} onchange={(e) => portscanProfiles.setActive((e.currentTarget as HTMLSelectElement).value)}>
        {#each $portscanProfiles as p (p.id)}
          <option value={p.id}>{p.builtin ? p.name : `★ ${p.name}`} ({p.tcp_ports.length}T/{p.udp_ports.length}U)</option>
        {/each}
      </select>
      <button class="icon-btn" title={$_('port_scan.manage_profiles')} onclick={() => showManager = true}>⚙</button>
      <input bind:value={newIp} placeholder={$_('port_scan.placeholder')} onkeydown={(e) => e.key === 'Enter' && addHost()} />
      <button class="primary" onclick={addHost}>{$_('port_scan.add')}</button>
    </div>
  </div>

  <div class="list">
    {#each [...$portscan.values()] as entry (entry.ip)}
      <div class="card">
        <div
          class="card-header"
          role="button"
          tabindex="0"
          onclick={() => portscan.toggle(entry.ip)}
          onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); portscan.toggle(entry.ip); } }}
        >
          <span class="chevron" class:expanded={entry.expanded}>▸</span>
          <span class="ip">{entry.ip}</span>
          {#if entry.scanning}
            <span class="badge scanning">{$_('port_scan.scanning')}</span>
          {:else if entry.error}
            <span class="badge err">{$_('port_scan.error')}</span>
          {:else}
            <span class="badge done">{$_('port_scan.open_count', { values: { n: openCount(entry) } })}</span>
          {/if}
          {#if entry.profileName}
            <span class="badge profile" title={$_('port_scan.scanned_with')}>★ {entry.profileName}</span>
          {/if}
          {#if entry.scannedAt && !entry.scanning}
            <span class="timestamp">{formatScannedAt(entry.scannedAt)}</span>
          {/if}
          <span class="spacer"></span>
          <button class="mini" onclick={(e) => { e.stopPropagation(); rescan(entry); }} title={$_('port_scan.rescan')}>⟳</button>
          <button class="mini" onclick={(e) => { e.stopPropagation(); removeHost(entry.ip); }} title={$_('port_scan.remove')}>✕</button>
        </div>

        {#if entry.expanded}
          <div class="card-body">
            <div class="entry-toolbar">
              <label class="entry-profile">
                <span class="entry-profile-label">{$_('port_scan.rescan_with')}</span>
                <select
                  value={entry.profileId ?? activeId}
                  disabled={entry.scanning}
                  onchange={(e) => rescanWith(entry.ip, (e.currentTarget as HTMLSelectElement).value)}
                >
                  {#each $portscanProfiles as p (p.id)}
                    <option value={p.id}>{p.builtin ? p.name : `★ ${p.name}`} ({p.tcp_ports.length}T/{p.udp_ports.length}U)</option>
                  {/each}
                </select>
              </label>
            </div>
            {#if entry.error}
              <div class="error">{entry.error}</div>
            {/if}

            <div class="section-title">{$_('port_scan.tcp')}</div>
            {#if entry.tcpResults.length === 0 && entry.scanning}
              <div class="pending">{$_('port_scan.pending')}</div>
            {:else}
              {@const openTcp = entry.tcpResults.filter(r => r.open)}
              {#if openTcp.length === 0}
                <div class="pending">{$_('port_scan.no_open')}</div>
              {:else}
                <div class="port-grid">
                  {#each openTcp as r}
                    <div class="port-row open">
                      <span class="dot green"></span>
                      <span class="port-num">{r.port}</span>
                      <span class="service">{r.service}</span>
                      <span class="status">{$_('port_scan.state_open')}</span>
                    </div>
                  {/each}
                </div>
              {/if}
            {/if}

            <div class="section-title">
              {$_('port_scan.udp')}
              <span class="hint">{$_('port_scan.udp_hint')}</span>
            </div>
            {#if entry.udpResults.length === 0 && entry.scanning}
              <div class="pending">{$_('port_scan.pending')}</div>
            {:else}
              {@const openUdp = entry.udpResults.filter(r => r.open)}
              {#if openUdp.length === 0}
                <div class="pending">{$_('port_scan.no_open')}</div>
              {:else}
                <div class="port-grid">
                  {#each openUdp as r}
                    <div class="port-row open">
                      <span class="dot green"></span>
                      <span class="port-num">{r.port}</span>
                      <span class="service">{r.service}</span>
                      <span class="status">{$_('port_scan.state_open')}</span>
                    </div>
                  {/each}
                </div>
              {/if}
            {/if}
          </div>
        {/if}
      </div>
    {:else}
      <p class="empty">{$_('port_scan.empty')}</p>
    {/each}
  </div>

  {#if showManager}
    <div class="modal-backdrop" role="button" tabindex="0"
      onclick={() => { showManager = false; editing = null; }}
      onkeydown={(e) => { if (e.key === 'Escape') { showManager = false; editing = null; } }}>
      <div class="modal" role="dialog" tabindex="-1"
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => e.stopPropagation()}>
        <div class="modal-head">
          <h2>{$_('port_scan.manage_profiles')}</h2>
          <button class="icon-btn" onclick={() => { showManager = false; editing = null; }}>✕</button>
        </div>

        {#if editing}
          <div class="editor">
            <label>
              <span>{$_('port_scan.profile_name')}</span>
              <input bind:value={editName} placeholder="My profile" />
            </label>
            <label>
              <span>{$_('port_scan.tcp_ports')}</span>
              <textarea bind:value={editTcpText} placeholder="22, 80, 443, 8000-8100"></textarea>
            </label>
            <label>
              <span>{$_('port_scan.udp_ports')}</span>
              <textarea bind:value={editUdpText} placeholder="53, 161, 500"></textarea>
            </label>
            <div class="editor-actions">
              <button onclick={() => editing = null}>{$_('port_scan.cancel')}</button>
              <button class="primary" onclick={saveEdit}>{$_('port_scan.save')}</button>
            </div>
          </div>
        {:else}
          <div class="profile-list">
            {#each $portscanProfiles as p (p.id)}
              <div class="profile-row">
                <div class="profile-meta">
                  <div class="profile-name">{p.builtin ? p.name : `★ ${p.name}`}</div>
                  <div class="profile-sub">{p.tcp_ports.length} TCP · {p.udp_ports.length} UDP</div>
                </div>
                <div class="profile-actions">
                  {#if !p.builtin}
                    <button class="mini" onclick={() => openEdit(p)}>✎</button>
                    <button class="mini" onclick={() => removeProfile(p)}>✕</button>
                  {:else}
                    <button class="mini" onclick={() => openEdit({ ...p, id: `custom:${Date.now()}`, name: `${p.name} copy`, builtin: false })}>⎘</button>
                  {/if}
                </div>
              </div>
            {/each}
            <button class="primary new-profile" onclick={openNew}>+ {$_('port_scan.new_profile')}</button>
          </div>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .page { padding: 24px; }
  .header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 20px; flex-wrap: wrap; gap: 12px; }
  h1 { font-size: 20px; font-weight: 700; }
  .add-row { display: flex; gap: 8px; }
  input { background: var(--ep-bg-tertiary); border: 1px solid var(--ep-border); border-radius: 6px; padding: 7px 12px; color: var(--ep-text-primary); font-family: var(--ep-font-mono); font-size: 13px; width: 200px; }
  button { padding: 7px 14px; border-radius: 6px; border: 1px solid var(--ep-border); background: var(--ep-bg-tertiary); color: var(--ep-text-primary); cursor: pointer; font-size: 13px; font-weight: 600; }
  button.primary { background: var(--ep-accent); border-color: var(--ep-accent); color: #fff; }
  .list { display: flex; flex-direction: column; gap: 10px; }
  .card {
    background: var(--ep-glass-bg);
    border: 1px solid var(--ep-glass-border);
    border-radius: var(--ep-radius-lg);
    overflow: hidden;
    transition: border-color 0.15s;
  }
  .card:hover { border-color: var(--ep-glass-border-strong); }
  .card-header {
    display: flex; align-items: center; gap: 12px;
    width: 100%; padding: 14px 16px;
    background: transparent; border: none; border-radius: 0;
    color: var(--ep-text-primary); cursor: pointer; text-align: left;
    font-size: 13px;
  }
  .card-header:hover { background: var(--ep-glass-bg-md); }
  .chevron { display: inline-block; transition: transform 120ms ease; color: var(--ep-text-muted); font-size: 12px; }
  .chevron.expanded { transform: rotate(90deg); }
  .ip { font-family: var(--ep-font-mono); font-weight: 700; font-size: 14px; }
  .spacer { flex: 1; }
  .badge { font-size: 11px; font-weight: 600; padding: 3px 8px; border-radius: 10px; }
  .badge.scanning { background: color-mix(in srgb, var(--ep-accent) 15%, transparent); color: var(--ep-accent); }
  .badge.done { background: color-mix(in srgb, var(--ep-success) 15%, transparent); color: var(--ep-success); }
  .badge.err { background: color-mix(in srgb, var(--ep-danger) 15%, transparent); color: var(--ep-danger); }
  .badge.profile { background: color-mix(in srgb, #06b6d4 15%, transparent); color: #06b6d4; }
  .timestamp { font-size: 11px; color: var(--ep-text-muted); font-family: var(--ep-font-mono); }
  .entry-toolbar { display: flex; justify-content: flex-end; padding-bottom: 10px; border-bottom: 1px dashed var(--ep-glass-border); margin-bottom: 10px; }
  .entry-profile { display: flex; align-items: center; gap: 8px; font-size: 11px; }
  .entry-profile-label { color: var(--ep-text-muted); text-transform: uppercase; letter-spacing: .5px; font-weight: 600; }
  .entry-profile select {
    background: var(--ep-bg-tertiary); border: 1px solid var(--ep-border);
    border-radius: 6px; padding: 5px 8px; color: var(--ep-text-primary);
    font-size: 11px; font-weight: 600; cursor: pointer;
  }
  .mini { padding: 4px 8px; font-size: 12px; border-radius: 6px; background: transparent; border: 1px solid var(--ep-border); color: var(--ep-text-secondary); }
  .mini:hover { background: var(--ep-bg-tertiary); color: var(--ep-text-primary); }
  .card-body { padding: 12px 16px 16px; border-top: 1px solid var(--ep-glass-border); }
  .section-title { font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: .5px; color: var(--ep-text-secondary); margin: 10px 0 8px; display: flex; gap: 8px; align-items: baseline; }
  .section-title .hint { font-size: 10px; text-transform: none; letter-spacing: 0; font-weight: 400; color: var(--ep-text-muted); }
  .port-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 6px; }
  .port-row {
    display: flex; align-items: center; gap: 8px; padding: 6px 10px;
    border-radius: var(--ep-radius-sm); background: var(--ep-glass-bg);
    border: 1px solid var(--ep-glass-border); font-size: 12px;
    transition: border-color 0.12s;
  }
  .port-row.open { border-color: var(--ep-success); }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--ep-border); flex-shrink: 0; }
  .dot.green { background: var(--ep-success); }
  .port-num { font-family: var(--ep-font-mono); font-weight: 700; min-width: 36px; }
  .service { flex: 1; color: var(--ep-text-secondary); }
  .status { font-size: 11px; font-weight: 600; color: var(--ep-text-muted); }
  .port-row.open .status { color: var(--ep-success); }
  .pending { font-size: 12px; color: var(--ep-text-muted); padding: 8px 0; }
  .error { color: var(--ep-danger); font-size: 12px; padding: 4px 0 8px; }
  .empty { color: var(--ep-text-muted); font-size: 14px; }

  .profile-select {
    background: var(--ep-bg-tertiary); border: 1px solid var(--ep-border);
    border-radius: 6px; padding: 7px 10px; color: var(--ep-text-primary);
    font-size: 12px; font-weight: 600; cursor: pointer;
  }
  .icon-btn {
    background: var(--ep-bg-tertiary); border: 1px solid var(--ep-border);
    color: var(--ep-text-secondary); padding: 6px 10px; border-radius: 6px;
    font-size: 14px; cursor: pointer;
  }
  .icon-btn:hover { color: var(--ep-text-primary); }

  .modal-backdrop {
    position: fixed; inset: 0; background: rgba(0,0,0,.55);
    display: flex; align-items: center; justify-content: center;
    z-index: 1000; padding: 20px;
  }
  .modal {
    background: var(--ep-bg-secondary); border: 1px solid var(--ep-glass-border-strong);
    border-radius: var(--ep-radius-lg); padding: 0; width: 480px; max-width: 100%;
    max-height: 80vh; overflow: auto;
  }
  .modal-head {
    display: flex; justify-content: space-between; align-items: center;
    padding: 16px 20px; border-bottom: 1px solid var(--ep-glass-border);
  }
  .modal-head h2 { font-size: 15px; font-weight: 700; margin: 0; }
  .profile-list { display: flex; flex-direction: column; gap: 6px; padding: 14px 20px 18px; }
  .profile-row {
    display: flex; align-items: center; justify-content: space-between;
    padding: 10px 12px; background: var(--ep-glass-bg);
    border: 1px solid var(--ep-glass-border); border-radius: var(--ep-radius-md);
    transition: border-color 0.12s;
  }
  .profile-row:hover { border-color: var(--ep-glass-border-strong); }
  .profile-name { font-size: 13px; font-weight: 700; }
  .profile-sub { font-size: 11px; color: var(--ep-text-muted); margin-top: 2px; }
  .profile-actions { display: flex; gap: 4px; }
  .new-profile { margin-top: 6px; }
  .editor { padding: 14px 20px 18px; display: flex; flex-direction: column; gap: 12px; }
  .editor label { display: flex; flex-direction: column; gap: 4px; font-size: 11px; font-weight: 600; color: var(--ep-text-secondary); text-transform: uppercase; letter-spacing: .5px; }
  .editor input, .editor textarea {
    background: var(--ep-bg-tertiary); border: 1px solid var(--ep-border);
    border-radius: 6px; padding: 7px 10px; color: var(--ep-text-primary);
    font-family: var(--ep-font-mono); font-size: 12px; width: 100%;
  }
  .editor textarea { resize: vertical; min-height: 60px; }
  .editor-actions { display: flex; justify-content: flex-end; gap: 8px; }
</style>
