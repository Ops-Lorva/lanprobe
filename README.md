<div align="center">

[🇫🇷 Français](README.fr.md) · 🇬🇧 English

# LanProbe

**Network monitoring and diagnostics — probes, and a hub to drive them**

*Interface Profiles · Ping Monitor · SLA · Discovery · Port Scan · Speed Test · Self-hosted Hub*

[![Latest release](https://img.shields.io/github/v/release/Benjamin-Chianese/lanprobe?label=release&style=flat-square)](https://github.com/Benjamin-Chianese/lanprobe/releases/latest)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-FFC131?logo=tauri&logoColor=white&style=flat-square)](https://tauri.app)
[![Rust](https://img.shields.io/badge/Rust-1.85+-CE422B?logo=rust&logoColor=white&style=flat-square)](https://rustlang.org)
[![Svelte](https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white&style=flat-square)](https://svelte.dev)
[![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-6366f1?style=flat-square)](#-compatibility)
[![License](https://img.shields.io/badge/License-MIT-22c55e?style=flat-square)](LICENSE)

</div>

---

## 🖧 What is LanProbe?

LanProbe replaces a handful of separate network utilities with one coherent tool, for people who switch interfaces often, debug connectivity, or watch several hosts at once.

It comes in **two pieces**, and the split is worth knowing before installing anything:

| | Role |
|---|---|
| **The probe** | Measures. Desktop app (Windows / macOS / Linux) or headless service. It pings, scans and speed-tests — from inside the network being watched. |
| **The hub** | Watches. A Docker container on your own machine, gathering every probe behind **one address and one authentication**. |

A probe on its own is perfectly usable: open the window, it measures. The hub earns its place as soon as there are several sites, several machines, or a need to look without sitting in front of the screen.

⚠️ **The direction of the connection matters.** The hub never reaches into a probe — it has no route to its network, and that is the point. The probe calls the hub, outbound. So there is **no inbound port to open** on the monitored network, and no VPN to set up.

---

## 🧩 Features

| Module | Description |
|--------|-------------|
| 🔀 **Interface profiles** | Named static-IP or DHCP configurations, applied in one click |
| 📡 **Ping Monitor** | Continuous ICMP watch over several hosts, live latency, configurable thresholds |
| 📊 **SLA export** | Uptime % per host, avg / min / max / P95 latency — CSV export |
| 🔍 **Network discovery** | Async CIDR sweep: IP, hostname, MAC address |
| 🔌 **Port Scan** | TCP scan with built-in profiles (common, web, full) and custom ones |
| ⚡ **Speed Test** | Ookla or iperf3, **bound to the selected interface** |
| 🛡️ **Internet status** | Dual ICMP + HTTP probe, public IP, uptime percentage |
| 🌐 **Self-hosted hub** | Sites, probes, accounts, roles, audit log, alerts, backups |
| 🔑 **Two-factor and passkeys** | TOTP everywhere, passkeys over HTTPS — with a rescue command for a lost phone |
| 👥 **Per-site scope** | An account can see only certain sites: show a client their fleet, not the others' |
| 🗄️ **Archiving** | A client you no longer serve leaves the fleet without anything being deleted — the SLA report stays exportable |
| 🎨 **Theme** | Dark / light / system, 6 accent palettes |

### The rule that governs everything: the selected interface

⚠️ **All of a probe's traffic leaves through the interface you picked.** The measurements, but also its heartbeat to the hub and the delivery of its readings.

This is not an implementation detail. You can have internet on `eth1` and none on `eth0`, and `eth0` is precisely the one under test. A probe that measured a dead link while reporting “all good” over another link would be asserting something it never checked.

The consequence to know: if the selected interface loses its address, the probe **stops beating** and turns “no news” in the fleet. That is the truth of the interface being watched, not a fault in the probe.

---

## 📦 Installation

### 💻 Desktop app

From the [releases](https://github.com/Benjamin-Chianese/lanprobe/releases/latest):

| Platform | File |
|---|---|
| Windows | `lanprobe_vX.Y.Z_x64-setup.exe` |
| macOS | `lanprobe_vX.Y.Z_universal.pkg` (signed + notarized) |
| Linux | `lanprobe_vX.Y.Z_amd64.deb` |

On macOS the `.pkg` provisions the sudoers rights needed to change an IP. On Linux the package sets `CAP_NET_RAW` and `CAP_NET_ADMIN` on the binary: raw ICMP pings and interface changes work without root.

---

### 🗄️ Headless probe on Debian / Ubuntu

For a Raspberry Pi, a VM, or a server with no desktop.

```bash
curl -fsSL -o install-server.sh https://raw.githubusercontent.com/Benjamin-Chianese/lanprobe/main/install-server.sh
sudo bash install-server.sh
```

⚠️ **The headless probe listens on no port.** It used to serve its own web interface over HTTPS on 8443, with its own accounts and self-signed certificate. That role now belongs to the hub: a probe exposing its own interface as well would be a second surface to secure for the very same measurements. So you drive it from the command line, and you look at it from the hub.

**Attaching.** In the hub, create an enrolment code (Fleet → “+” on a site), then:

```bash
sudo -u lanprobe lanprobe-server --config-dir /var/lib/lanprobe \
     enroll --hub https://hub.example.com --code A1B2-C3D4

sudo systemctl enable --now lanprobe-server
```

The code lasts 15 minutes and works once. The probe shows up in the fleet on its first heartbeat, so under a minute.

**Commands:**

```bash
lanprobe-server run         # measure and send, until Ctrl-C (default action)
lanprobe-server enroll      # attach to a hub
lanprobe-server interfaces  # list the network interfaces this machine exposes
lanprobe-server status      # show the attachment
lanprobe-server forget      # detach — the hub keeps the measurements
```

**Pick the interface to measure.** With no screen there is nowhere to choose
one, so `--interface` does it, on `run` and on `enroll`:

```bash
lanprobe-server interfaces              # en0  address 10.0.8.235/24  gateway 10.0.8.1  actif
lanprobe-server run --interface en0     # remembered — no need to repeat it
```

⚠️ **Without it the probe follows the system's default route and reports
neither gateway nor local addresses.** The SLA report's *Gateway* column and
the network labels then stay empty for good — the probe looks healthy and
tells you nothing about where it sits. An unknown name fails immediately and
lists the ones that exist; a name that is remembered but currently missing (an
unplugged adapter, a stopped VPN) is kept and warned about, never dropped in
silence.

The listing states each link: `actif`, `activé, sans porteuse` (enabled but the
cable is out — check the cable, not the config), or `inactif`. ⚠️ On Linux this
is read from the kernel; on macOS and Windows it is still reported as active
regardless, so trust it only on Linux for now.

`--config-dir` applies to every command. Default: `~/.config/lanprobe`, and `/var/lib/lanprobe` for the systemd service.

For scripted deployment, `--username`, `--password` and `--site` replace `--code`. Avoid it on a machine you do not control: a compromised host should not receive the hub's credentials.

**Firewall:** nothing to open inbound. The probe only needs to reach the hub's address outbound.

**A hub with a self-signed certificate.** The probe does not accept it blindly
— it **pins** it. Enrol without saying anything and the command stops, showing
the fingerprint it saw:

```
Le certificat de hub.example.com ne se vérifie pas.
  empreinte SHA-256 : 39:73:1A:…:7F:0D
  --pin 39:73:1A:…:7F:0D
```

Compare it with the one your hub displays, then re-run with `--pin`. From then
on that certificate and no other is accepted.

⚠️ Only what needs pinning is pinned: a certificate that verifies on its own is
left alone, otherwise a renewal at a public authority would cut the probe off
every three months.

⚠️ `--allow-self-signed` still exists and still works, for probes already
deployed with it. Prefer `--pin`: the old flag accepts **any** certificate, so
anyone on the path can impersonate the hub and walk away with the probe's
token.

**Sealing key.** The probe's local secrets — its hub token — are encrypted at rest. The key is created on first start inside the config directory. To supply your own (immutable container, secret manager):

```bash
head -c 32 /dev/urandom | base64        # 32 random bytes
# then, as a systemd drop-in:
# [Service]
# Environment=LANPROBE_SECRET_KEY=<base64-32-bytes>
```

---

### 🐳 Self-hosted hub (Docker)

One container: the hub, its SQLite database, and an embedded InfluxDB. **We host nothing** — it all runs on a machine you control.

```bash
git clone https://github.com/Benjamin-Chianese/lanprobe.git && cd lanprobe
docker compose up -d
docker compose logs lanprobe-web | grep -i "setup token"
```

Open the hub, paste the token, create the administrator account — the token is consumed on first use. Then **Enrol a probe**: the hub gives you a short code, valid 15 minutes. In LanProbe, **Settings → Server connection**, enter the hub address and that code. No password ever reaches the monitored machine.

Accounts carry a role (`admin` / `operator` / `viewer`), every action is written to the audit log, and offline / online transitions can go out by e-mail or webhook (Slack, Discord, ntfy…). The [full API lives in the contract](docs/lanprobe-web-contrat.md).

**Two traps that cost ten minutes:**
- the setup token is readable **only** in `docker logs` — the interface never shows it;
- if that machine was ever reached over HTTPS, the browser keeps a `Secure` cookie that silently blocks the later HTTP session: sign-in appears to succeed, then bounces back to the login screen, **with no error**. Private window, or clear the site's cookies.

The hub serves **in plain HTTP by default**, meant to live behind your own reverse proxy. A switch under **Settings → General** — or `--tls` — makes it terminate TLS itself with a self-signed certificate, fine on a LAN. ⚠️ It takes effect **on restart**, the address changes, and probes already enrolled on the old one will need theirs corrected. Passkeys require it (or a proxy): browsers refuse WebAuthn outside a secure context. The InfluxDB port (`8086`) is closed by default: only open it to point Grafana at it — probes never need it.

The hub carries **its own version**, independent of the app.

#### Backup

The hub backs itself up, Sonarr-style: a complete archive in its backup folder, and restoring means handing the file back. Configured under **Settings → Storage** (interval, archives kept).

⚠️ **An archive contains `secret.key`**, which decrypts notification passwords and webhook URLs. Treat that folder as a secret.

From the command line, for a cron on the host:

```bash
docker exec lanprobe-web lanprobe-web backup     # produces an archive, applies retention
docker exec lanprobe-web lanprobe-web backups    # lists them
```

Restoring is done **with the container running**, then restarted: the InfluxDB
half talks to the series database, which has to answer. See “Restoring an
archive” below.

#### 🚑 Getting back in when you are locked out

This is self-hosted: **there is no recovery e-mail and no support**. Every way
back goes through the container — that is, through access to the machine,
which is the only thing that can stand in for the credentials you lost.

Each one is written to the audit log: they bypass sign-in, so they must leave
a trace.

| Situation | Command |
|---|---|
| Lost administrator password | `docker exec lanprobe-web lanprobe-web reset-password admin <new>` |
| Lost phone, second factor in the way | `docker exec lanprobe-web lanprobe-web disable-totp <account>` |
| Account disabled by mistake | `reset-password` re-enables it on the way |
| No administrator left | `reset-password <account> <password> --promote` |

⚠️ **`disable-totp` also drops the secret**, not just the flag. An old code
must not come back to life on a QR you believe is new: turning it on again
means scanning afresh.

⚠️ **Passkeys have no rescue command, deliberately.** Register more than one —
the laptop *and* the phone — or keep a working password. A single passkey does
not replace the password.

⚠️ **An unreadable second factor closes the door on purpose.** Restore a volume
without its `secret.key` and the hub still knows the account requires a second
factor but can no longer verify codes: it **refuses** rather than falling back
to the password alone, and prints the command above. Letting it through would
switch the protection off in silence, on the very day you have reason to
distrust it.

#### Restoring an archive

```bash
# 1. the container must be RUNNING: restoring the series talks to InfluxDB
docker cp my-archive.zip lanprobe-web:/backup/
docker exec lanprobe-web lanprobe-web restore my-archive.zip --force

# 2. restart: the running process still serves the old database
docker restart lanprobe-web
```

⚠️ **`--force` is required** as soon as the volume already holds data: a
restore that overwrites without asking is not a restore, it is an accident.

⚠️ **The previous state is moved aside**, not deleted:
`avant-restauration-<date>` in the hub's volume.

⚠️ An archive produced by a **newer** hub is refused. Its schema is ahead of
this binary, and migrations do not run backwards.

---

## 🔧 Building from source

**Prerequisites**

- [Rust](https://rustup.rs/) ≥ 1.85 (edition 2024)
- [Node.js](https://nodejs.org/) ≥ 18
- **Linux desktop:** `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev`
- **Linux server only:** `libssl-dev pkg-config` (no GUI dependencies)
- **macOS:** `xcode-select --install`
- **Windows:** [WebView2](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) (preinstalled on Windows 11)

```bash
git clone https://github.com/Benjamin-Chianese/lanprobe.git
cd lanprobe
npm install
```

```bash
npm run tauri build                      # desktop app
cargo build --release -p lanprobe-server # headless probe, no GUI
npm run build:web && cargo build --release -p lanprobe-web  # hub
```

⚠️ `npm run build:web` **before** the hub: the interface is embedded in the binary, and a hub built without it starts up and then answers an explicit error in place of the page.

---

## 🛠️ Development

```bash
npm run tauri dev        # desktop, hot reload
npm run dev:web          # hub interface, proxied to a local hub
npm run check            # TypeScript / Svelte, app
npm test                 # svelte-check + Vitest, hub interface
cargo test --workspace   # Rust
```

⚠️ **There is no test CI.** The only workflow is triggered by a tag and merely builds. `cargo test` and `npm test` are **manual** steps, to run before deploying. What each suite covers, and why the interface tests exist at all, is written up in § 21 of the [contract](docs/lanprobe-web-contrat.md).

---

## 🏗️ Technical stack

```
Probes    →  Rust (Tauri 2 · tokio · reqwest) — no HTTP server
Hub       →  Rust (axum · rusqlite · InfluxDB 2)
Interface →  Svelte 5 + TypeScript, two separate frontends
Theme     →  CSS custom properties · dark / light / system · 6 palettes
i18n      →  svelte-i18n — English · French · Spanish, on both sides
Bundles   →  NSIS .exe · .dmg / .pkg · .deb · headless .deb · Docker image
```

### Cargo workspace

```
lanprobe/
├── src-tauri/                  # Tauri shell — commands, app lifecycle
├── crates/
│   ├── lanprobe-core/          # Measurement: ping, discovery, ports, speedtest, SLA
│   ├── lanprobe-server/        # Probe core: scheduler, export buffer, hub attachment
│   └── lanprobe-web/           # Hub: API, SQLite, InfluxDB, backups
├── src/                        # App interface (Svelte 5)
└── web-ui/                     # Hub interface (Svelte 5), embedded in its binary
```

⚠️ `lanprobe-server` is **no longer a server** despite its name: it listens on no port. It carries the measurement scheduler, the export buffer and the hub attachment, shared between the desktop app and the headless binary.

---

## 🖧 Compatibility

| OS | Version | Architecture |
|----|---------|--------------|
| Windows | 10, 11 | x64 |
| macOS | 12 Monterey+ | Intel · Apple Silicon · universal |
| Linux (desktop) | Debian 12+ · Ubuntu 22.04+ | x64 |
| Linux (headless probe) | Debian 11+ · Ubuntu 20.04+ · any systemd distro | x64 |
| Hub | Any machine with Docker | x64 · arm64 |

---

## 🚀 Release

One GitHub Actions workflow builds every platform in parallel and publishes a single GitHub Release:

| Job | Runner | Artifacts |
|-----|--------|-----------|
| `build-linux` | `ubuntu-22.04` | `lanprobe_vX.Y.Z_amd64.deb` |
| `build-linux-server` | `ubuntu-24.04` | `lanprobe-server_vX.Y.Z_amd64.deb` |
| `build-windows` | `windows-latest` | `lanprobe_vX.Y.Z_x64-setup.exe` |
| `build-macos` | `macos-latest` | `universal.dmg` + `universal.pkg` (signed + notarized) |
| `release` | `ubuntu-22.04` | collects artifacts, publishes the Release |

⚠️ **Two products, two tags.** The app and the hub do not ship together and
do not share a version — a `v1.0.0` tag would not say which one it means.

```bash
git tag app-v2.1.0 && git push origin app-v2.1.0   # application
git tag hub-1.0.0  && git push origin hub-1.0.0    # hub (Docker image)
```

The version comes from the tag; it is not written in `Cargo.toml`, which stays
at `0.0.0` for the app.

⚠️ The older `v2.1.0` form is still accepted for the app, and will remain so
while 2.0.x installs are out there: their embedded updater recognises only that
form, and dropping it would cut them off from updates **with no message at
all**. Published files always keep the `lanprobe_v2.1.0_…` shape, whatever the
tag prefix.

The hub publishes a Docker image to `ghcr.io`, pinned to its version. No
`latest`: an image that changes under a `docker compose pull` is exactly how
you skip a major version without noticing.

---

## 🗺️ Roadmap

- [x] Interface profiles (static IP / DHCP)
- [x] Multi-host ping monitor with latency charts
- [x] Network discovery (CIDR — IP / hostname / MAC)
- [x] TCP port scan, built-in and custom profiles
- [x] SLA — uptime %, avg / min / max / P95 latency, CSV export
- [x] Speed test bound to the selected interface (Ookla + iperf3)
- [x] Dark / light / system theme, 6 palettes
- [x] i18n — English, French, Spanish
- [x] Signed + notarized macOS `.pkg` with sudoers provisioning
- [x] Headless `.deb` with systemd service and capabilities
- [x] **Self-hosted hub** — Docker, embedded InfluxDB, sites, accounts, roles, audit
- [x] **Alerts** — e-mail and webhook, subscription per site and per probe
- [x] **Backup and restore** of the hub, automatic
- [x] **All traffic bound to the selected interface**, heartbeat included
- [x] Removal of the probe's web server — one place to secure
- [ ] The five per-probe tabs in the hub, and remote triggering
- [ ] Per-site scoping on accounts
- [ ] 2FA and passkeys on hub accounts

---

## 🤝 Contributing

Pull requests are welcome. For a significant change, open an issue first to discuss the approach.

---

<div align="center">
<sub>Built with Tauri · Rust · Svelte</sub>
</div>
