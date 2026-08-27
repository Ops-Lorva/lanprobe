<div align="center">

🇫🇷 Français · [🇬🇧 English](README.md)

# LanProbe

**Monitoring et diagnostic réseau — application desktop et serveur headless**

*Profils réseau · Ping Monitor · SLA · Découverte réseau · Port Scan · Speed Test · Mode serveur web*

[![Dernière version](https://img.shields.io/github/v/release/Benjamin-Chianese/lanprobe?label=release&style=flat-square)](https://github.com/Benjamin-Chianese/lanprobe/releases/latest)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-FFC131?logo=tauri&logoColor=white&style=flat-square)](https://tauri.app)
[![Rust](https://img.shields.io/badge/Rust-1.85+-CE422B?logo=rust&logoColor=white&style=flat-square)](https://rustlang.org)
[![Svelte](https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white&style=flat-square)](https://svelte.dev)
[![Plateforme](https://img.shields.io/badge/Plateforme-Windows%20%7C%20macOS%20%7C%20Linux-6366f1?style=flat-square)](#compatibilité)
[![Licence](https://img.shields.io/badge/Licence-MIT-22c55e?style=flat-square)](LICENSE)

</div>

---

## 🖧 Qu'est-ce que LanProbe ?

LanProbe remplace une poignée d'utilitaires réseau séparés par une interface cohérente. Conçu pour les ingénieurs qui changent fréquemment d'interface, déboguent des problèmes de connectivité, ou ont besoin de surveiller plusieurs hôtes simultanément.

- 🔀 **Changer de profil réseau en un clic** — fini les saisies manuelles d'IP statiques dans les dialogs système
- 📡 **Surveiller plusieurs hôtes en temps réel** avec historique de latence et statistiques SLA
- 🔍 **Scanner le réseau** pour découvrir les machines sur le sous-réseau
- ⚡ **Tester le débit** lié à une interface spécifique — sans surprise de routage OS
- 🗄️ **Déployer en headless** sur un serveur Debian ou Raspberry Pi — accès à l'UI complète depuis n'importe quel navigateur du LAN

---

## 🧩 Fonctionnalités

| Module | Description |
|--------|-------------|
| 🔀 **Profils réseau** | Enregistrer des configurations IP statique ou DHCP nommées, les appliquer en un clic |
| 📡 **Ping Monitor** | Surveillance ICMP continue de plusieurs hôtes, graphique de latence en temps réel, seuils d'alerte configurables |
| 📊 **Export SLA** | Uptime % par hôte, avg / min / max / P95 de latence — exportable en CSV |
| 🔍 **Découverte réseau** | Scan CIDR asynchrone rapide retournant IP, hostname et adresse MAC des hôtes actifs |
| 🔌 **Port Scan** | Scan TCP avec profils intégrés (common, web, full) et profils personnalisés |
| ⚡ **Speed Test** | Test de débit Ookla CLI lié à l'interface sélectionnée via `IP_BOUND_IF` / `SO_BINDTODEVICE` |
| 🌐 **Mode serveur web** | Expose l'UI LanProbe complète en HTTPS sur le LAN — application desktop ou binaire headless standalone |
| 🛡️ **Statut internet** | Double sonde (ICMP + HTTP) avec IP publique et pourcentage d'uptime |
| 🎨 **Palettes de couleurs** | 6 palettes d'accent (Indigo, Cyan, Emerald, Rose, Amber, Slate) — mode sombre et clair |

---

## 📦 Installation

### 💻 Application desktop

Les installeurs pré-compilés sont publiés sur **[GitHub Releases](https://github.com/Benjamin-Chianese/lanprobe/releases/latest)**.

| OS | Fichier | Notes |
|----|---------|-------|
| Windows 10 / 11 | `lanprobe_vX.Y.Z_x64-setup.exe` | Installeur NSIS, droits UAC requis pour la config réseau |
| macOS (Intel + Apple Silicon) | `lanprobe_vX.Y.Z_universal.pkg` | Signé + notarisé, provisionnement sudoers automatique |
| macOS (Intel + Apple Silicon) | `lanprobe_vX.Y.Z_universal.dmg` | Glisser dans Applications, mot de passe demandé à la première application de profil |
| Linux (Debian / Ubuntu) | `lanprobe_vX.Y.Z_amd64.deb` | Application desktop avec WebKit2GTK |

L'application embarque un **auto-updater** — les mises à jour suivantes se font en un clic depuis la bannière de notification.

> **macOS** — utilisez l'installeur `.pkg` pour la meilleure expérience : il provisionne l'entrée sudoers automatiquement pour que l'application des profils réseau ne demande jamais de mot de passe.

---

### 🗄️ Serveur headless sur Debian / Ubuntu (sans interface graphique)

`lanprobe-server` est un binaire standalone qui sert l'UI web LanProbe complète en HTTPS. Il ne nécessite aucun environnement de bureau et tourne comme un service systemd.

#### 1 — Installer ou mettre à jour en une ligne

```bash
curl -fsSL https://raw.githubusercontent.com/Benjamin-Chianese/lanprobe/main/install-server.sh | sudo bash
```

Le script récupère automatiquement la dernière version, installe le `.deb` et redémarre le service s'il était déjà en cours d'exécution.
Pour installer une version précise :

```bash
curl -fsSL https://raw.githubusercontent.com/Benjamin-Chianese/lanprobe/main/install-server.sh | sudo bash -s -- --version v0.6.10
```

Ou télécharger et exécuter localement :

```bash
curl -fsSL -o install-server.sh https://raw.githubusercontent.com/Benjamin-Chianese/lanprobe/main/install-server.sh
sudo bash install-server.sh
```

Le paquet crée automatiquement :
- Un utilisateur système dédié `lanprobe`
- Les capabilities `CAP_NET_RAW` + `CAP_NET_ADMIN` sur le binaire (ICMP + config interface sans root)
- L'enregistrement et le démarrage de `lanprobe-server.service` via systemd

#### 2 — Vérifier le service

```bash
sudo systemctl status lanprobe-server
# En écoute sur https://0.0.0.0:8443 par défaut

# Suivre les logs
sudo journalctl -u lanprobe-server -f
```

#### 3 — Premier démarrage

Au premier accès, l'UI affiche un **écran de configuration** pour créer le compte administrateur. Ouvrir un navigateur depuis n'importe quelle machine du même LAN :

```
https://<ip-du-serveur>:8443
```

L'écran demande aussi un **token de setup**, généré au premier démarrage et lisible sur le serveur :

```bash
sudo cat /var/lib/lanprobe/setup-token
# ou : sudo journalctl -u lanprobe-server | grep -i token
```

> **Pourquoi un token ?** Sans lui, n'importe qui atteignant l'instance entre le démarrage du service et votre première connexion pourrait s'attribuer le compte admin. Une fois le compte créé, le token est consommé, la route d'installation est fermée **côté serveur** définitivement, et `/api/setup` répond `409` à toute tentative ultérieure — il n'existe aucun chemin pour « rejouer » l'installation.

> Le serveur génère un certificat TLS auto-signé au premier démarrage. Le navigateur affichera un avertissement de sécurité — accepter l'exception. Le certificat est stocké dans `/var/lib/lanprobe/`.

#### 4 — Ouvrir le pare-feu (si nécessaire)

```bash
sudo ufw allow 8443/tcp
```

#### ⚙️ Configuration

Le fichier de service `/lib/systemd/system/lanprobe-server.service` utilise ces valeurs par défaut :

```
--host 0.0.0.0   # écoute sur toutes les interfaces
--port 8443      # port HTTPS
--config-dir /var/lib/lanprobe   # stockage users + certificat TLS
```

Pour changer le port, éditer le service et recharger :

```bash
sudo systemctl edit lanprobe-server
# Ajouter :
# [Service]
# ExecStart=
# ExecStart=/usr/bin/lanprobe-server --host 0.0.0.0 --port 9443 --config-dir /var/lib/lanprobe

sudo systemctl daemon-reload && sudo systemctl restart lanprobe-server
```

#### Mettre à jour

```bash
curl -LO https://github.com/Benjamin-Chianese/lanprobe/releases/latest/download/lanprobe-server_vX.Y.Z_amd64.deb
sudo dpkg -i lanprobe-server_vX.Y.Z_amd64.deb
# dpkg arrête, remplace et redémarre le service automatiquement
```

#### Désinstaller

```bash
sudo apt remove lanprobe-server
```

#### 🔒 Modèle de sécurité — ce qui est protégé, et contre quoi

LanProbe est **auto-hébergé uniquement**. Aucun service central, aucun annuaire de comptes, aucun relais : votre compte et vos données vivent dans *votre* instance. Cela veut dire aussi que la frontière de sécurité vous appartient. Voici la mesure exacte, sans la survendre.

| Donnée | Au repos | Remarques |
|---|---|---|
| Mots de passe utilisateur (`users.json`) | **Hachés**, argon2id + sel par utilisateur | Jamais chiffrés — un mot de passe réversible est une faille, pas une fonctionnalité. Fichier en `0600`. |
| Jeton / mot de passe InfluxDB (`app_config.json`) | **Chiffrés**, AES-256-GCM, nonce tiré à chaque écriture | Fichier en `0600`. Les champs non secrets (URL, org, bucket, identifiant) restent lisibles pour pouvoir diagnostiquer une instance. |
| Jetons de session | En mémoire uniquement | 32 octets aléatoires, TTL 7 jours, cookie `HttpOnly` + `SameSite=Strict` + `Secure`. Perdus au redémarrage, volontairement. |

**Où vit la clé de chiffrement — à lire avant de lui faire confiance.** Par défaut, la clé est un fichier `secret.key` (`0600`) posé dans le même `--config-dir` que `app_config.json`. Il faut être lucide sur ce que cela apporte : cela protège une **copie du seul fichier de config** — une sauvegarde qui fuit, un volume exporté, une config collée dans un ticket. Cela ne protège **pas** contre quelqu'un qui a déjà la machine ou le volume complet : il lit les deux fichiers.

Pour un vrai gain, gardez la clé hors du volume en la passant par l'environnement :

```bash
# 32 octets aléatoires, en base64
openssl rand -base64 32

# drop-in systemd, secret Docker, ou votre gestionnaire de secrets
sudo systemctl edit lanprobe-server
# [Service]
# Environment=LANPROBE_SECRET_KEY=<base64-32-octets>
```

Quand `LANPROBE_SECRET_KEY` est définie, **aucun fichier de clé n'est écrit**. Conservez une copie de cette clé : la perdre rend les identifiants InfluxDB stockés illisibles, il faudra les ressaisir. Si aucune clé utilisable n'est disponible, le serveur **refuse d'écrire un secret** plutôt que de le rétrograder silencieusement en clair.

**Transport.** Le HTTPS intégré s'appuie sur un certificat **auto-signé** — suffisant sur un LAN, pas un substitut à un vrai certificat. Si vous exposez l'instance au-delà de votre LAN, placez-la derrière votre propre reverse proxy (Caddy, Traefik, nginx) et laissez-le terminer le TLS avec un certificat de confiance. LanProbe n'obtient ni ne renouvelle de certificat à votre place.

---

### 🐳 Hub web auto-hébergé (Docker) — pour un parc de sondes

🚧 **En développement**, construit contre [`docs/lanprobe-web-contrat.md`](docs/lanprobe-web-contrat.md) — le packaging Docker ci-dessous est construit et testé ; le binaire du hub et son interface web arrivent par des branches séparées, pas encore fusionnées.

Plusieurs sondes à superviser ? Le hub est **un seul conteneur Docker** — l'API d'enrôlement et son InfluxDB embarqué — auquel vos sondes remontent leurs mesures. **Nous n'hébergeons rien** : tout tourne sur une machine que vous contrôlez, de bout en bout.

#### Mise en route

Rien à éditer — on clone, on démarre, on ouvre le navigateur :

```bash
git clone https://github.com/Benjamin-Chianese/lanprobe.git && cd lanprobe
docker compose up -d
docker compose logs lanprobe-web | grep -i "jeton de configuration"
```

Ouvrez `https://<cette-machine>:8443` (certificat auto-signé — acceptez l'avertissement du navigateur, comme pour le serveur headless ci-dessus) et utilisez ce jeton pour créer le compte admin. Il est consommé au premier usage et ne réapparaît jamais.

**Tout le reste vit dans l'application, pas dans `docker-compose.yml`.** Une fois le compte admin créé, l'organisation Influx, le bucket, la rétention et l'URL annoncée aux sondes sont des réglages que vous changez depuis l'interface — ils vivent dans la base SQLite du hub, survivent à un redéploiement, et ne demandent jamais de toucher un fichier `.env` ni de redémarrer le conteneur. Le hub démarre déjà configuré contre l'InfluxDB qu'il vient d'initialiser ; un **bouton de test** existe à côté du réglage d'URL annoncée si vous remappez un jour le port publié et voulez vérifier que les sondes peuvent effectivement la joindre. Réduire la rétention efface les données au-delà de la nouvelle fenêtre — l'interface demande confirmation et nomme la quantité d'historique que vous vous apprêtez à perdre.

#### Enrôler une sonde

Pointez une sonde (application desktop ou `lanprobe-server`) vers l'URL du hub avec vos identifiants admin. Le hub renvoie un jeton propre à cette sonde et des coordonnées d'écriture InfluxDB restreintes à ce seul bucket — une sonde compromise peut écrire, mais ne peut ni lire ni effacer les mesures des autres. Voir le [contrat d'interface](docs/lanprobe-web-contrat.md) pour l'échange exact.

#### Sauvegarder

Deux volumes, deux rythmes différents — ne jamais en retirer un sans l'autre, ce sont les deux moitiés d'un même état :

| Volume | Contient | Fréquence de sauvegarde |
|---|---|---|
| `lanprobe_data` | Compte admin, jetons des sondes (hachés), jeton de configuration, certificat TLS du hub | Petit — sauvegardez-le souvent, ça ne coûte rien |
| `influxdb_data` | Mesures de séries temporelles | Peut atteindre plusieurs Go — sauvegardez selon la politique de rétention qui vous convient |

```bash
docker compose stop lanprobe-web
docker run --rm -v lanprobe_data:/from -v "$PWD":/to alpine tar czf /to/lanprobe_data.tar.gz -C /from .
docker run --rm -v influxdb_data:/from -v "$PWD":/to alpine tar czf /to/influxdb_data.tar.gz -C /from .
docker compose start lanprobe-web
```

> Le conteneur ne supprime ni n'écrase jamais ses propres données — ni au redémarrage, ni sur une initialisation interrompue en cours de route. Si InfluxDB et le jeton du hub se désynchronisent (premier démarrage coupé net), `docker logs lanprobe-web` explique précisément ce qui s'est passé et quoi faire — il ne devine jamais à votre place.

---

### 🌐 Mode serveur web (application desktop)

L'application desktop peut également agir comme serveur — l'activer depuis **Paramètres → Mode serveur**. Cela diffuse les données en direct depuis votre machine desktop vers n'importe quel navigateur du LAN sans installer de paquet séparé.

---

## 🔧 Compiler depuis les sources

**Prérequis**

- [Rust](https://rustup.rs/) ≥ 1.85 (edition 2024)
- [Node.js](https://nodejs.org/) ≥ 18
- **Linux desktop :** `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev`
- **Linux serveur uniquement :** `libssl-dev pkg-config` (pas de dépendances GUI)
- **macOS :** `xcode-select --install`
- **Windows :** [WebView2](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) (pré-installé sur Windows 11)

```bash
git clone https://github.com/Benjamin-Chianese/lanprobe.git
cd lanprobe
npm install
```

```bash
# Application desktop (Tauri)
npm run tauri build

# Binaire serveur headless uniquement (sans dépendances GUI)
npm run build                          # compile le frontend (embarqué dans le binaire serveur)
cargo build -p lanprobe-server --release
# Binaire → target/release/lanprobe-server
```

---

## 🛠️ Développement

```bash
# Mode dev desktop avec hot-reload
npm run tauri dev

# Vérification TypeScript / Svelte
npm run check

# Tests unitaires Rust
cargo test -p lanprobe-core

# Lancer le serveur headless en local
npm run build
cargo run -p lanprobe-server -- --host 0.0.0.0 --port 8443
```

---

## 🏗️ Stack technique

```
Backend   →  Rust (Tauri 2 · tokio · reqwest · axum)
Frontend  →  Svelte 5 + TypeScript
Icônes    →  SVG inline (style Lucide, sans dépendance runtime)
Thème     →  CSS custom properties · adaptatif OS + override manuel · 6 palettes
Stockage  →  JSON via tauri-plugin-store (desktop) / /var/lib/lanprobe (serveur)
i18n      →  svelte-i18n — Anglais · Français · Espagnol
Bundles   →  .exe NSIS · .dmg / .pkg · .deb · .deb headless
```

### Workspace Cargo

```
lanprobe/
├── src-tauri/                  # Shell Tauri — enregistrement des commandes, cycle de vie app
├── crates/
│   ├── lanprobe-core/          # Logique async partagée : ping, discovery, ports, speedtest, SLA
│   └── lanprobe-server/        # Serveur HTTPS headless standalone (UI servie sur le LAN)
└── src/                        # Frontend Svelte 5 (embarqué dans desktop et serveur)
    └── lib/
        ├── components/         # Un composant par module
        ├── stores/             # Stores Svelte (profils, monitoring, paramètres)
        └── i18n/               # Fichiers de traduction en / fr / es
```

---

## 🖧 Compatibilité

| OS | Version | Architecture |
|----|---------|--------------|
| Windows | 10, 11 | x64 |
| macOS | 12 Monterey+ | Intel · Apple Silicon · universal |
| Linux (desktop) | Debian 12+ · Ubuntu 22.04+ | x64 |
| Linux (serveur) | Debian 11+ · Ubuntu 20.04+ · toute distro systemd | x64 |

---

## 🚀 Pipeline CI / Release

Un workflow GitHub Actions compile toutes les plateformes en parallèle et publie une seule GitHub Release :

| Job | Runner | Artefacts |
|-----|--------|-----------|
| `build-linux` | `ubuntu-22.04` | `lanprobe_vX.Y.Z_amd64.deb` |
| `build-linux-server` | `ubuntu-24.04` | `lanprobe-server_vX.Y.Z_amd64.deb` |
| `build-windows` | `windows-latest` | `lanprobe_vX.Y.Z_x64-setup.exe` |
| `build-macos` | `macos-latest` | `universal.dmg` + `universal.pkg` (signé + notarisé) |
| `release` | `ubuntu-22.04` | collecte les artefacts · publie la GitHub Release |

Créer une release en poussant un tag de version :

```bash
git tag v1.0.0 && git push origin v1.0.0
```

---

## 🗺️ Feuille de route

- [x] Gestion des profils réseau (IP statique / DHCP)
- [x] Ping monitor multi-hôtes en temps réel avec graphiques de latence
- [x] Découverte réseau (scan CIDR — IP / hostname / MAC)
- [x] Port scan TCP avec profils intégrés et personnalisés
- [x] Monitoring SLA — uptime %, avg / min / max / P95 latence
- [x] Export SLA en CSV
- [x] Speed test lié à l'interface sélectionnée (Ookla + iperf3)
- [x] UI Glass & Depth — thème sombre / clair / système
- [x] Mode serveur web — partage de l'UI en HTTPS sur le LAN
- [x] `.deb` headless avec service systemd, capabilities, démarrage automatique
- [x] i18n — Anglais, Français, Espagnol
- [x] `.pkg` macOS signé + notarisé avec provisionnement sudoers
- [x] 6 palettes de couleurs (mode sombre + clair)
- [ ] Hub web auto-hébergé — Docker, InfluxDB embarqué, enrôlement d'un parc de sondes

---

## 🤝 Contribuer

Les pull requests sont les bienvenues. Pour les changements significatifs, merci d'ouvrir une issue au préalable pour discuter de l'approche.

---

<div align="center">
<sub>Construit avec Tauri · Rust · Svelte</sub>
</div>
