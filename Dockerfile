# syntax=docker/dockerfile:1.7
#
# Image du hub LanProbe auto-hébergé (lanprobe-web) + InfluxDB embarqué dans
# le même conteneur — décision explicite du propriétaire du projet : un seul
# conteneur à monter, pas une pile à assembler soi-même.
#
# ⚠️ Ce Dockerfile est construit CONTRE docs/lanprobe-web-contrat.md. Au
# moment où il a été écrit, ni crates/lanprobe-web ni web-ui/ n'existent
# encore dans ce dépôt (ils arrivent par les branches feat/web-hub et
# feat/web-ui). L'étape "build Rust" échouera donc tant que ces deux
# livraisons n'auront pas fusionné — c'est attendu, voir le rapport de
# packaging pour le détail des hypothèses faites ici et à recouper.
#
# Toutes les images de base sont épinglées par tag ET par digest — jamais de
# `latest` (une image `latest` a déjà cassé un service en production sur ce
# projet).

########################################################################
# Étape 1 — build de l'interface web (web-ui/ → web-ui/dist)
########################################################################
FROM node:22.23.2-bookworm-slim@sha256:83f487e0a63425e5b4d146fb5e5be574bcbe1b7b843d3ebafdd95eaf7767a7e5 AS web-ui-builder

WORKDIR /src

# L'interface du hub n'a pas son propre package.json : elle est construite
# depuis la racine (`npm run build:web` → `vite build --config
# web-ui/vite.config.ts`), et elle importe la palette et deux composants du
# frontend desktop via des alias Vite. Une seule définition des couleurs pour
# les deux surfaces — d'où la copie de `src/` en plus de `web-ui/`.
COPY package.json package-lock.json ./
RUN npm ci

COPY web-ui/ ./web-ui/
COPY src/ ./src/
COPY svelte.config.js ./
RUN npm run build:web

########################################################################
# Étape 2 — build du binaire Rust du hub (cargo build -p lanprobe-web)
########################################################################
FROM rust:1.98.0-slim-bookworm@sha256:1469a27c125cb5a3aebfa4f4e4665d935b02fb72cc093b2c974b3d740e43f157 AS rust-builder

# Toolchain C minimale : les crates avec composants natifs déjà présentes
# dans ce workspace (ring, rustls, argon2 — voir
# crates/lanprobe-server/Cargo.toml) en ont besoin. À ajuster si le Cargo.toml
# réel de lanprobe-web introduit d'autres dépendances système (ex.
# libsqlite3-dev si le stockage SQLite du contrat n'est pas "bundled").
# libssl-dev : `webauthn-rs` (clés d'accès) passe par `openssl-sys`, qui se
# lie à la libssl du système. Le paquet `openssl` de l'étape finale apporte la
# bibliothèque d'exécution correspondante ; les deux images étant épinglées à
# la même Debian 12 par empreinte, l'ABI ne peut pas diverger entre elles.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        pkg-config \
        libssl-dev \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Tout le workspace Cargo doit être présent pour que `cargo build -p
# lanprobe-web --locked` puisse résoudre le graphe de dépendances contre
# Cargo.lock, mais seule lanprobe-web (+ ses dépendances) sera réellement
# compilée : les build.rs des autres membres (notamment src-tauri) ne
# s'exécutent que pour les paquets effectivement construits.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src-tauri ./src-tauri

# rust-embed embarque le frontend DANS le binaire au moment de la compilation
# (déjà le mécanisme de crates/lanprobe-server, cf.
# crates/lanprobe-server/src/assets.rs : `#[folder =
# "$CARGO_MANIFEST_DIR/../../build"]`). Par symétrie, lanprobe-web attend très
# probablement web-ui/dist au même niveau relatif — d'où cette copie avant le
# `cargo build`, et non dans l'image finale.
COPY --from=web-ui-builder /src/web-ui/dist ./web-ui/dist

# Les libellés du classeur SLA sont ceux du navigateur : `report_i18n.rs` les
# embarque par `include_str!("../../../web-ui/src/lib/i18n/*.json")` plutôt
# que d'en garder une copie, pour qu'un renommage de clé casse la compilation
# au lieu de laisser diverger les intitulés du classeur et ceux de
# l'interface. Ces catalogues doivent donc être présents à la COMPILATION, au
# même chemin relatif que dans le dépôt (ici /build/web-ui/src/lib/i18n) —
# `web-ui/dist` ne contient que le résultat du build de l'interface, pas ses
# sources. Leur absence a fait échouer la construction de l'image 1.6.0.
# ⚠️ Le dossier des catalogues seul : copier tout `web-ui/src` alourdirait le
# contexte sans rien apporter au binaire.
COPY web-ui/src/lib/i18n ./web-ui/src/lib/i18n

# `web-ui/dist` doit exister ici : le binaire l'embarque via rust-embed.
RUN cargo build --release --locked -p lanprobe-web --bin lanprobe-web

########################################################################
# Étape 3 — récupération d'InfluxDB 2 (serveur + CLI), versions épinglées
# et vérifiées par sha256 (binaires téléchargés et testés manuellement avant
# d'écrire ces valeurs — voir le rapport de packaging)
########################################################################
FROM debian:12.15-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171 AS influx-fetch

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
    && rm -rf /var/lib/apt/lists/*

ARG INFLUXD_VERSION=2.9.1
ARG INFLUXD_SHA256=762e4fc825c4386e0c5138e7c3f91fc778081db2bada1ec47066e786bf55d9ff
ARG INFLUX_CLI_VERSION=2.8.0
ARG INFLUX_CLI_SHA256=feaa32e02c99815414574265d5471c4f2550b07aef6f8b19907d8a1af98e5d61

WORKDIR /fetch

RUN curl -fsSL -o influxd.tar.gz \
        "https://dl.influxdata.com/influxdb/releases/influxdb2-${INFLUXD_VERSION}_linux_amd64.tar.gz" \
    && echo "${INFLUXD_SHA256}  influxd.tar.gz" | sha256sum -c - \
    && tar xzf influxd.tar.gz \
    && mkdir -p /out \
    && mv "influxdb2-${INFLUXD_VERSION}/influxd" /out/influxd

RUN curl -fsSL -o influx-cli.tar.gz \
        "https://dl.influxdata.com/influxdb/releases/influxdb2-client-${INFLUX_CLI_VERSION}-linux-amd64.tar.gz" \
    && echo "${INFLUX_CLI_SHA256}  influx-cli.tar.gz" | sha256sum -c - \
    && mkdir -p cli && tar xzf influx-cli.tar.gz -C cli \
    && mv cli/influx /out/influx

RUN chmod 755 /out/influxd /out/influx

########################################################################
# Étape 4 — image finale, minimale
########################################################################
FROM debian:12.15-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171 AS final

LABEL org.opencontainers.image.title="lanprobe-web" \
      org.opencontainers.image.description="Hub LanProbe auto-hébergé — enrôlement des sondes + InfluxDB embarqué. Auto-hébergé uniquement : rien n'est hébergé par le projet." \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.source="https://github.com/Benjamin-Chianese/lanprobe"

# tini    : PID 1 correct — reap des zombies + transmission immédiate des
#           signaux, pour que `docker stop` n'attende jamais les 10s de grâce
#           avant SIGKILL. Voir docker/run.sh pour la justification complète
#           du choix de superviseur applicatif (au-dessus de tini).
# gosu    : abandon des privilèges root → utilisateur applicatif, après la
#           seule étape qui en a besoin (chown des volumes, docker/entrypoint.sh).
# openssl : génère le certificat TLS auto-signé d'InfluxDB au premier démarrage.
# curl    : utilisé par le HEALTHCHECK pour interroger le hub, et par
#           run.sh pour attendre qu'InfluxDB réponde réellement.
RUN apt-get update && apt-get install -y --no-install-recommends \
        tini \
        gosu \
        openssl \
        curl \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 lanprobe \
    && useradd  --system --uid 10001 --gid lanprobe --no-create-home \
                --shell /usr/sbin/nologin lanprobe

COPY --from=rust-builder /build/target/release/lanprobe-web /usr/local/bin/lanprobe-web
COPY --from=influx-fetch /out/influxd                        /usr/local/bin/influxd
COPY --from=influx-fetch /out/influx                         /usr/local/bin/influx

COPY docker/entrypoint.sh  /usr/local/bin/entrypoint.sh
COPY docker/run.sh         /usr/local/bin/run.sh
COPY docker/healthcheck.sh /usr/local/bin/healthcheck.sh
RUN chmod 755 \
        /usr/local/bin/lanprobe-web \
        /usr/local/bin/entrypoint.sh \
        /usr/local/bin/run.sh \
        /usr/local/bin/healthcheck.sh

# Créés ici pour fixer les permissions par défaut de l'image ; réaffirmés par
# entrypoint.sh à chaque démarrage car un volume Docker nommé est monté
# par-dessus avec ses propres permissions (root:root) tant qu'il est vide.
RUN mkdir -p /data/lanprobe /data/influxdb \
    && chown -R lanprobe:lanprobe /data

ENV LANPROBE_WEB_HOST=0.0.0.0 \
    LANPROBE_WEB_PORT=8080 \
    LANPROBE_WEB_CONFIG_DIR=/data/lanprobe \
    LANPROBE_WEB_BACKUP_DIR=/backup \
    LANPROBE_INFLUX_CLI=/usr/local/bin/influx \
    INFLUX_PORT=8086 \
    INFLUX_ORG=lanprobe \
    INFLUX_BUCKET=lanprobe

# Trois volumes distincts et non un seul.
#
# /data/lanprobe (comptes, jetons, base SQLite, cert TLS du hub, secret.key)
#   est petit et change peu.
# /data/influxdb (séries temporelles) peut grossir de plusieurs Go.
# /backup porte les archives de sauvegarde, que le hub produit lui-même.
#   Il est séparé pour une raison précise : une sauvegarde qui vit dans le
#   volume qu'elle sauvegarde ne protège de rien. L'utilisateur est invité à
#   le pointer sur un autre disque depuis le compose.
#
# ⚠️ Une archive contient `secret.key`, qui déchiffre les identifiants de
# notification du hub : /backup est un dossier de secrets.
RUN mkdir -p /backup && chown lanprobe:lanprobe /backup
VOLUME ["/data/lanprobe", "/data/influxdb", "/backup"]

# 8080 : interface web du hub, en clair — à placer derrière le reverse
#        proxy de l'utilisateur. LANPROBE_WEB_TLS=true pour du TLS auto-signé.
#        C'est aussi par ce port que les sondes envoient leurs mesures : le
#        hub relaie vers InfluxDB, elles ne lui parlent jamais directement.
# 8086 : InfluxDB. Utile seulement pour brancher un Grafana ; laissé fermé
#        par défaut dans le compose, les sondes n'en ont pas besoin.
EXPOSE 8080 8086

# Le conteneur démarre root le temps strictement nécessaire à
# docker/entrypoint.sh (préparer les volumes) ; tini reste PID 1 en toutes
# circonstances, y compris après l'abandon de privilèges par gosu.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint.sh"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=45s --retries=3 \
    CMD ["/usr/local/bin/healthcheck.sh"]
