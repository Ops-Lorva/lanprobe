<div align="center">

🇫🇷 Français · [🇬🇧 English](README.md)

# LanProbe

**Supervision et diagnostic réseau — des sondes, et un hub pour les piloter**

*Profils réseau · Ping Monitor · SLA · Découverte · Port Scan · Speed Test · Hub auto-hébergé*

[![Dernière version](https://img.shields.io/github/v/release/Benjamin-Chianese/lanprobe?label=release&style=flat-square)](https://github.com/Benjamin-Chianese/lanprobe/releases/latest)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-FFC131?logo=tauri&logoColor=white&style=flat-square)](https://tauri.app)
[![Rust](https://img.shields.io/badge/Rust-1.85+-CE422B?logo=rust&logoColor=white&style=flat-square)](https://rustlang.org)
[![Svelte](https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white&style=flat-square)](https://svelte.dev)
[![Plateforme](https://img.shields.io/badge/Plateforme-Windows%20%7C%20macOS%20%7C%20Linux-6366f1?style=flat-square)](#-compatibilité)
[![Licence](https://img.shields.io/badge/Licence-MIT-22c55e?style=flat-square)](LICENSE)

</div>

---

## 🖧 Qu'est-ce que LanProbe ?

LanProbe remplace une poignée d'utilitaires réseau séparés par un outil cohérent, pour les gens qui changent souvent d'interface, déboguent de la connectivité, ou surveillent plusieurs hôtes à la fois.

Il se présente en **deux morceaux**, et il faut connaître la répartition avant d'installer quoi que ce soit :

| | Rôle |
|---|---|
| **La sonde** | Mesure. Application de bureau (Windows / macOS / Linux) ou service headless. C'est elle qui pingue, scanne, teste le débit — depuis l'intérieur du réseau observé. |
| **Le hub** | Regarde. Un conteneur Docker chez vous, qui rassemble toutes les sondes derrière **une seule adresse et une seule authentification**. |

Une sonde seule est parfaitement utilisable : ouvrez la fenêtre, elle mesure. Le hub devient utile dès qu'il y a plusieurs sites, plusieurs machines, ou qu'on veut regarder sans être devant l'écran.

⚠️ **Le sens de la connexion compte.** Le hub ne joint jamais une sonde — il n'a aucune route vers son réseau, et c'est voulu. C'est la sonde qui appelle le hub, en sortie. Il n'y a donc **aucun port à ouvrir** sur le réseau surveillé, ni VPN à monter.

---

## 🧩 Fonctionnalités

| Module | Description |
|--------|-------------|
| 🔀 **Profils réseau** | Configurations IP statique ou DHCP nommées, appliquées en un clic |
| 📡 **Ping Monitor** | Surveillance ICMP continue de plusieurs hôtes, latence en direct, seuils configurables |
| 📊 **Export SLA** | Uptime % par hôte, latence moyenne / min / max / P95 — export CSV |
| 🔍 **Découverte réseau** | Scan CIDR asynchrone : IP, nom d'hôte, adresse MAC |
| 🔌 **Port Scan** | Scan TCP avec profils intégrés (common, web, full) et personnalisés |
| ⚡ **Speed Test** | Ookla ou iperf3, **lié à l'interface sélectionnée** |
| 🛡️ **Statut internet** | Double sonde ICMP + HTTP, IP publique, pourcentage d'uptime |
| 🌐 **Hub auto-hébergé** | Sites, sondes, comptes, rôles, journal d'audit, alertes, sauvegardes |
| 🎨 **Thème** | Sombre / clair / système, 6 palettes d'accent |

### La règle qui gouverne tout : l'interface sélectionnée

⚠️ **Tout le trafic d'une sonde sort par l'interface que vous avez choisie.** Les mesures, mais aussi son battement de cœur vers le hub et l'envoi de ses relevés.

Ce n'est pas un détail d'implémentation. On peut avoir internet sur `eth1` et pas sur `eth0`, et `eth0` est justement celui qu'on cherche à éprouver. Une sonde qui mesurerait un lien mort tout en répondant « je vais bien » par un autre lien affirmerait quelque chose qu'elle n'a pas vérifié.

Conséquence à connaître : si l'interface sélectionnée n'a plus d'adresse, la sonde **cesse de battre** et passe « sans nouvelles » dans le parc. C'est la vérité de l'interface observée, pas une panne de la sonde.

---

## 📦 Installation

### 💻 Application de bureau

Téléchargez depuis les [releases](https://github.com/Benjamin-Chianese/lanprobe/releases/latest) :

| Plateforme | Fichier |
|---|---|
| Windows | `lanprobe_vX.Y.Z_x64-setup.exe` |
| macOS | `lanprobe_vX.Y.Z_universal.pkg` (signé + notarisé) |
| Linux | `lanprobe_vX.Y.Z_amd64.deb` |

Sur macOS, le `.pkg` provisionne les droits sudoers nécessaires au changement d'IP. Sur Linux, le paquet pose `CAP_NET_RAW` et `CAP_NET_ADMIN` sur le binaire : les ping ICMP bruts et les changements d'interface fonctionnent sans être root.

---

### 🗄️ Sonde headless sur Debian / Ubuntu

Pour un Raspberry Pi, une VM ou un serveur sans interface graphique.

```bash
curl -fsSL -o install-server.sh https://raw.githubusercontent.com/Benjamin-Chianese/lanprobe/main/install-server.sh
sudo bash install-server.sh
```

⚠️ **La sonde headless n'écoute sur aucun port.** Elle servait autrefois sa propre interface web en HTTPS sur 8443, avec ses comptes et son certificat auto-signé. Ce rôle appartient maintenant au hub : une sonde qui exposerait la sienne en plus serait une seconde surface à sécuriser pour montrer les mêmes mesures. On la pilote donc en ligne de commande, et on la consulte depuis le hub.

**Rattachement.** Dans le hub, créez un code d'enrôlement (Parc → « + » sur un site), puis :

```bash
sudo -u lanprobe lanprobe-server --config-dir /var/lib/lanprobe \
     enroll --hub https://hub.exemple.fr --code A1B2-C3D4

sudo systemctl enable --now lanprobe-server
```

Le code vaut 15 minutes et ne sert qu'une fois. La sonde apparaît dans le parc au premier battement, donc en moins d'une minute.

**Commandes :**

```bash
lanprobe-server run       # mesure et envoie, jusqu'à Ctrl-C (action par défaut)
lanprobe-server enroll    # rattache à un hub
lanprobe-server status    # affiche le rattachement
lanprobe-server forget    # détache — le hub garde les mesures
```

`--config-dir` s'applique à toutes les commandes. Défaut : `~/.config/lanprobe`, et `/var/lib/lanprobe` pour le service systemd.

Pour un déploiement scripté, `--username`, `--password` et `--site` remplacent `--code`. À éviter sur une machine que vous ne contrôlez pas : un hôte compromis ne devrait pas recevoir les identifiants du hub.

**Pare-feu :** rien à ouvrir en entrée. Il faut seulement que la sonde puisse joindre l'adresse du hub en sortie.

**Clé de scellement.** Les secrets locaux de la sonde — son jeton de hub — sont chiffrés au repos. La clé est créée au premier démarrage dans le dossier de configuration. Pour la fournir vous-même (conteneur immuable, gestionnaire de secrets) :

```bash
head -c 32 /dev/urandom | base64        # 32 octets aléatoires
# puis, en drop-in systemd :
# [Service]
# Environment=LANPROBE_SECRET_KEY=<base64-32-octets>
```

---

### 🐳 Hub auto-hébergé (Docker)

Un seul conteneur : le hub, sa base SQLite, et un InfluxDB embarqué. **Nous n'hébergeons rien** — tout tourne sur une machine que vous contrôlez.

```bash
git clone https://github.com/Benjamin-Chianese/lanprobe.git && cd lanprobe
docker compose up -d
docker compose logs lanprobe-web | grep -i "jeton de configuration"
```

Ouvrez le hub, collez le jeton, créez le compte administrateur — le jeton est consommé au premier usage. Puis **Enrôler une sonde** : le hub donne un code court, valable 15 minutes. Dans LanProbe, **Réglages → Connexion au serveur**, saisissez l'adresse du hub et ce code. Aucun mot de passe n'atteint la machine surveillée.

Les comptes portent un rôle (`admin` / `operator` / `viewer`), chaque action est écrite au journal d'audit, et les bascules hors ligne / en ligne peuvent partir par e-mail ou par webhook (Slack, Discord, ntfy…). L'[API complète est décrite dans le contrat](docs/lanprobe-web-contrat.md).

**Deux pièges qui coûtent dix minutes :**
- le jeton de configuration ne se lit **que** dans `docker logs` — l'interface ne l'affiche jamais ;
- si cette machine a déjà été jointe en HTTPS, le navigateur garde un cookie `Secure` qui bloque silencieusement la connexion en HTTP ensuite : la connexion semble aboutir puis revient à l'écran de login, **sans erreur**. Fenêtre privée, ou vider les cookies du site.

Le hub sert **en clair par défaut**, pensé pour vivre derrière votre reverse proxy. `--tls` produit un certificat auto-signé, suffisant sur un LAN. Le port InfluxDB (`8086`) est fermé par défaut : ne l'ouvrez que pour brancher Grafana dessus, les sondes n'en ont jamais besoin.

Le hub a **sa propre version**, indépendante de l'application.

#### Sauvegarde

Le hub sauvegarde tout seul, comme Sonarr : une archive complète dans son dossier de sauvegarde, et pour restaurer on redonne le fichier. Réglable dans **Réglages → Stockage** (cadence, nombre d'archives conservées).

⚠️ **Une archive contient `secret.key`**, qui déchiffre les mots de passe de notification et les URL de webhook. Traitez ce dossier comme un secret.

En ligne de commande, pour un cron sur l'hôte :

```bash
docker exec lanprobe-web lanprobe-web backup     # produit une archive, applique la rétention
docker exec lanprobe-web lanprobe-web backups    # liste
```

La restauration se fait **hub arrêté** — c'est le seul moment où elle prend effet sans redémarrage.

#### Mot de passe administrateur perdu

C'est de l'auto-hébergé : il n'y a pas d'e-mail de récupération. La porte de secours passe par le conteneur.

```bash
docker exec lanprobe-web lanprobe-web reset-password admin <nouveau-mot-de-passe>
```

Le compte est réactivé s'il était désactivé, et l'opération est écrite au journal d'audit — cette commande contourne la connexion, elle doit laisser une trace.

---

## 🔧 Compiler depuis les sources

**Prérequis**

- [Rust](https://rustup.rs/) ≥ 1.85 (edition 2024)
- [Node.js](https://nodejs.org/) ≥ 18
- **Linux desktop :** `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev`
- **Linux serveur uniquement :** `libssl-dev pkg-config` (aucune dépendance GUI)
- **macOS :** `xcode-select --install`
- **Windows :** [WebView2](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) (pré-installé sur Windows 11)

```bash
git clone https://github.com/Benjamin-Chianese/lanprobe.git
cd lanprobe
npm install
```

```bash
npm run tauri build                      # application de bureau
cargo build --release -p lanprobe-server # sonde headless, sans GUI
npm run build:web && cargo build --release -p lanprobe-web  # hub
```

⚠️ `npm run build:web` **avant** le hub : l'interface est embarquée dans le binaire, et un hub construit sans elle démarre puis répond une erreur explicite à la place de la page.

---

## 🛠️ Développement

```bash
npm run tauri dev        # bureau, avec rechargement à chaud
npm run dev:web          # interface du hub, proxy vers un hub local
npm run check            # TypeScript / Svelte, application
npm test                 # svelte-check + Vitest, interface du hub
cargo test --workspace   # Rust
```

⚠️ **Il n'y a pas de CI de test.** Le seul workflow est déclenché par un tag et ne fait que construire. `cargo test` et `npm test` sont des gestes **manuels**, à faire avant de déployer. Ce que couvre chaque suite, et pourquoi les tests de l'interface existent, est décrit au § 20 du [contrat](docs/lanprobe-web-contrat.md).

---

## 🏗️ Stack technique

```
Sondes    →  Rust (Tauri 2 · tokio · reqwest) — aucun serveur HTTP
Hub       →  Rust (axum · rusqlite · InfluxDB 2)
Interface →  Svelte 5 + TypeScript, deux frontends distincts
Thème     →  Propriétés CSS · sombre / clair / système · 6 palettes
i18n      →  svelte-i18n — anglais · français · espagnol, des deux côtés
Bundles   →  .exe NSIS · .dmg / .pkg · .deb · .deb headless · image Docker
```

### Workspace Cargo

```
lanprobe/
├── src-tauri/                  # Shell Tauri — commandes, cycle de vie
├── crates/
│   ├── lanprobe-core/          # Mesure : ping, découverte, ports, speedtest, SLA
│   ├── lanprobe-server/        # Cœur de la sonde : ordonnanceur, tampon, rattachement au hub
│   └── lanprobe-web/           # Hub : API, SQLite, InfluxDB, sauvegardes
├── src/                        # Interface de l'application (Svelte 5)
└── web-ui/                     # Interface du hub (Svelte 5), embarquée dans son binaire
```

⚠️ `lanprobe-server` **n'est plus un serveur** malgré son nom : il n'écoute sur aucun port. Il porte l'ordonnanceur des mesures, le tampon d'export et le rattachement au hub, partagés entre l'application de bureau et le binaire headless.

---

## 🖧 Compatibilité

| OS | Version | Architecture |
|----|---------|--------------|
| Windows | 10, 11 | x64 |
| macOS | 12 Monterey+ | Intel · Apple Silicon · universal |
| Linux (bureau) | Debian 12+ · Ubuntu 22.04+ | x64 |
| Linux (sonde headless) | Debian 11+ · Ubuntu 20.04+ · toute distro systemd | x64 |
| Hub | Toute machine avec Docker | x64 · arm64 |

---

## 🚀 Release

Un workflow GitHub Actions compile toutes les plateformes en parallèle et publie une seule GitHub Release :

| Job | Runner | Artefacts |
|-----|--------|-----------|
| `build-linux` | `ubuntu-22.04` | `lanprobe_vX.Y.Z_amd64.deb` |
| `build-linux-server` | `ubuntu-24.04` | `lanprobe-server_vX.Y.Z_amd64.deb` |
| `build-windows` | `windows-latest` | `lanprobe_vX.Y.Z_x64-setup.exe` |
| `build-macos` | `macos-latest` | `universal.dmg` + `universal.pkg` (signé + notarisé) |
| `release` | `ubuntu-22.04` | collecte les artefacts, publie la Release |

```bash
git tag v2.1.0 && git push origin v2.1.0
```

La version vient du tag ; elle n'est pas écrite dans `Cargo.toml`, qui reste à `0.0.0`.

---

## 🗺️ Feuille de route

- [x] Profils réseau (IP statique / DHCP)
- [x] Ping monitor multi-hôtes avec graphiques de latence
- [x] Découverte réseau (CIDR — IP / nom d'hôte / MAC)
- [x] Port scan TCP, profils intégrés et personnalisés
- [x] SLA — uptime %, latence moyenne / min / max / P95, export CSV
- [x] Speed test lié à l'interface sélectionnée (Ookla + iperf3)
- [x] Thème sombre / clair / système, 6 palettes
- [x] i18n — anglais, français, espagnol
- [x] `.pkg` macOS signé + notarisé, provisionnement sudoers
- [x] `.deb` headless avec service systemd et capabilities
- [x] **Hub auto-hébergé** — Docker, InfluxDB embarqué, sites, comptes, rôles, audit
- [x] **Alertes** — e-mail et webhook, abonnement par site et par sonde
- [x] **Sauvegarde et restauration** du hub, automatiques
- [x] **Tout le trafic lié à l'interface sélectionnée**, battement de cœur compris
- [x] Retrait du serveur web de la sonde — un seul endroit à sécuriser
- [ ] Les cinq onglets par sonde dans le hub, et le déclenchement à distance
- [ ] Périmètre par site sur les comptes
- [ ] 2FA et clés d'accès sur les comptes du hub

---

## 🤝 Contribuer

Les pull requests sont les bienvenues. Pour un changement significatif, ouvrez une issue au préalable pour discuter de l'approche.

---

<div align="center">
<sub>Construit avec Tauri · Rust · Svelte</sub>
</div>
