#!/usr/bin/env bash
# run.sh — superviseur applicatif, exécuté en tant qu'utilisateur non-root
# `lanprobe` (voir entrypoint.sh). Responsable de :
#   1. générer le certificat TLS d'InfluxDB au tout premier démarrage ;
#   2. démarrer InfluxDB et attendre qu'il réponde réellement (pas juste que
#      le process existe) ;
#   3. initialiser InfluxDB (org, bucket, jeton opérateur) UNE SEULE FOIS,
#      au tout premier démarrage — jamais ensuite, quoi qu'il arrive ;
#   4. démarrer le hub — qui lit lui-même le jeton opérateur dans le volume,
#      voir le contrat §7 ("on configure dans l'application, pas dans
#      l'environnement") ;
#   5. transmettre proprement les signaux d'arrêt aux deux processus, et
#      arrêter l'un si l'autre meurt.
#
# Choix de superviseur : ni s6-overlay ni supervisord. Deux processus
# longue-durée seulement, avec une logique de démarrage déjà nécessairement
# sur-mesure (init conditionnelle d'InfluxDB) — un script bash avec `trap` +
# boucle `kill -0` fait tout ce qu'il faut, sans dépendance supplémentaire ni
# système de service déclaratif à apprendre pour un mainteneur qui n'en a
# besoin que pour deux binaires.
set -uo pipefail  # pas de -e : les échecs des sous-process sont gérés explicitement

LANPROBE_DATA="${LANPROBE_WEB_CONFIG_DIR:-/data/lanprobe}"
INFLUX_DATA="/data/influxdb"
INFLUX_TLS_DIR="${INFLUX_DATA}/tls"
INFLUX_TOKEN_FILE="${LANPROBE_DATA}/influx-operator-token"

INFLUX_PORT="${INFLUX_PORT:-8086}"
INFLUX_ORG="${INFLUX_ORG:-lanprobe}"
INFLUX_BUCKET="${INFLUX_BUCKET:-lanprobe}"

log() { printf '[lanprobe-hub] %s\n' "$*" >&2; }

# ── 1. Certificat TLS d'InfluxDB — généré une seule fois, jamais régénéré ───
# InfluxDB est servi en HTTPS sur la machine même : il lui faut son propre
# certificat, indépendant de celui du hub. Ses seuls clients sont le hub
# lui-même — les sondes envoient leurs mesures au hub, qui relaie — et un
# éventuel Grafana branché sur le port 8086.
# Vous pouvez fournir le vôtre en déposant cert.pem/key.pem dans ce dossier
# avant le premier démarrage — ce script ne les touchera jamais s'ils existent.
#
# LANPROBE_ADVERTISE_HOST est entièrement optionnelle : si vous la
# fournissez, elle est ajoutée au SAN du certificat, ce qui évite un
# avertissement de nom d'hôte en plus de l'avertissement « auto-signé »
# quand vous ouvrez InfluxDB depuis un navigateur ou un Grafana. Sans elle,
# tout fonctionne.
mkdir -p "$INFLUX_TLS_DIR"
if [ ! -f "${INFLUX_TLS_DIR}/cert.pem" ] || [ ! -f "${INFLUX_TLS_DIR}/key.pem" ]; then
    log "Génération du certificat TLS auto-signé d'InfluxDB (premier démarrage)."
    san="DNS:localhost,IP:127.0.0.1"
    if [ -n "${LANPROBE_ADVERTISE_HOST:-}" ]; then
        if [[ "$LANPROBE_ADVERTISE_HOST" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
            san="${san},IP:${LANPROBE_ADVERTISE_HOST}"
        else
            san="${san},DNS:${LANPROBE_ADVERTISE_HOST}"
        fi
    fi
    umask 077
    openssl req -x509 -nodes -newkey rsa:2048 -days 3650 \
        -keyout "${INFLUX_TLS_DIR}/key.pem" -out "${INFLUX_TLS_DIR}/cert.pem" \
        -subj "/CN=lanprobe-hub" -addext "subjectAltName=${san}" \
        >/dev/null 2>&1
else
    log "Certificat TLS InfluxDB existant conservé (aucune régénération)."
fi

# ── 2. Démarrage d'InfluxDB ──────────────────────────────────────────────────
influxd \
    --bolt-path "${INFLUX_DATA}/influxd.bolt" \
    --engine-path "${INFLUX_DATA}/engine" \
    --http-bind-address ":${INFLUX_PORT}" \
    --tls-cert "${INFLUX_TLS_DIR}/cert.pem" \
    --tls-key "${INFLUX_TLS_DIR}/key.pem" &
INFLUXD_PID=$!

log "Attente de la disponibilité réelle d'InfluxDB (pas juste du process)..."
influx_ready=false
for _ in $(seq 1 60); do
    if ! kill -0 "$INFLUXD_PID" 2>/dev/null; then
        log "ERREUR : influxd s'est arrêté avant d'être prêt."
        exit 1
    fi
    if curl -fsSk --max-time 2 "https://127.0.0.1:${INFLUX_PORT}/health" 2>/dev/null | grep -q '"status":"pass"'; then
        influx_ready=true
        break
    fi
    sleep 1
done
if [ "$influx_ready" != true ]; then
    log "ERREUR : InfluxDB ne répond toujours pas après 60s."
    kill -TERM "$INFLUXD_PID" 2>/dev/null || true
    exit 1
fi
log "InfluxDB est prêt."

# ── 3. Initialisation InfluxDB — au tout premier démarrage SEULEMENT ────────
# Le marqueur d'initialisation est la présence du fichier de jeton lui-même :
# pas de fichier "témoin" séparé qui pourrait se désynchroniser de la réalité.
if [ ! -f "$INFLUX_TOKEN_FILE" ]; then
    log "Premier démarrage détecté — initialisation d'InfluxDB (org=${INFLUX_ORG}, bucket=${INFLUX_BUCKET})."
    OPERATOR_TOKEN="$(openssl rand -hex 32)"
    SETUP_PASSWORD="$(openssl rand -base64 24)"

    if influx setup \
            --host "https://127.0.0.1:${INFLUX_PORT}" \
            --skip-verify \
            --username "lanprobe-operator" \
            --password "$SETUP_PASSWORD" \
            --org "$INFLUX_ORG" \
            --bucket "$INFLUX_BUCKET" \
            --retention 0 \
            --token "$OPERATOR_TOKEN" \
            --force \
            --configs-path "${INFLUX_DATA}/.influx-cli-configs" \
            >/dev/null 2>&1; then
        # Écriture atomique : fichier temporaire puis rename, pour ne jamais
        # laisser un fichier de jeton tronqué en cas de coupure au mauvais moment.
        umask 077
        printf '%s' "$OPERATOR_TOKEN" > "${INFLUX_TOKEN_FILE}.tmp"
        mv "${INFLUX_TOKEN_FILE}.tmp" "$INFLUX_TOKEN_FILE"
        log "InfluxDB initialisé. Jeton opérateur persisté dans ${INFLUX_TOKEN_FILE}."
    else
        # `influx setup` refuse tout net si l'instance est déjà initialisée
        # (vérifié manuellement : message "instance has already been set up",
        # code de sortie 1, AUCUNE donnée touchée). On tombe dans cette
        # branche uniquement si un démarrage précédent a réussi le setup côté
        # InfluxDB mais a été interrompu (kill -9, coupure) avant d'écrire le
        # fichier de jeton local — une fenêtre de quelques millisecondes.
        log "ERREUR : InfluxDB semble déjà initialisé mais aucun jeton n'est"
        log "persisté dans ${INFLUX_TOKEN_FILE}. C'est le signe d'un arrêt brutal"
        log "entre la fin du setup InfluxDB et l'écriture du jeton local."
        log "Aucune donnée existante n'a été modifiée ni effacée par ce script."
        log "Comme l'org/bucket créés dans cette fenêtre sont encore vides,"
        log "le plus sûr est de repartir propre : supprimez ENSEMBLE (jamais"
        log "l'un sans l'autre) les volumes influxdb_data et lanprobe_data,"
        log "puis relancez 'docker compose up'."
        kill -TERM "$INFLUXD_PID" 2>/dev/null || true
        exit 1
    fi
else
    log "InfluxDB déjà initialisé — jeton opérateur existant réutilisé."
fi

# ── 4. Démarrage du hub ──────────────────────────────────────────────────────
# Contrat §7 : « on configure dans l'application, pas dans l'environnement ».
# Le jeton opérateur N'EST PLUS exporté — le hub le lit lui-même dans
# <config-dir>/influx-operator-token (écrit à l'étape 3 ci-dessus, avec les
# mêmes permissions et la même écriture atomique qu'avant). Ce fichier ne
# sort jamais de ce script.
#
# INFLUX_URL/INFLUX_ORG/INFLUX_BUCKET/INFLUX_ADVERTISE_URL restent acceptées
# par le hub comme *valeurs initiales* au premier démarrage seulement (dès
# qu'un réglage existe en base, la base gagne — contrat §7). On les fournit
# quand même ici, sans que l'utilisateur n'ait rien à définir : ce sont
# exactement les valeurs que l'étape 3 vient de créer dans InfluxDB, donc le
# hub démarre déjà correctement configuré sans passage obligé par l'interface.
export INFLUX_URL="https://127.0.0.1:${INFLUX_PORT}"
export INFLUX_ORG
export INFLUX_BUCKET

# LANPROBE_ADVERTISE_HOST est désormais optionnelle de bout en bout (contrat
# §7, "L'URL annoncée n'est pas l'URL interne") : à défaut, le hub déduit
# l'URL Influx à annoncer aux sondes depuis l'en-tête Host de la requête
# d'enrôlement, et elle reste corrigeable dans l'interface à tout moment
# (avec un bouton de test). On ne journalise donc plus d'avertissement si
# elle est absente — ce n'est plus une anomalie, c'est le cas nominal.
if [ -n "${LANPROBE_ADVERTISE_HOST:-}" ]; then
    export INFLUX_ADVERTISE_URL="https://${LANPROBE_ADVERTISE_HOST}:${INFLUX_PORT}"
fi

lanprobe-web \
    --host "${LANPROBE_WEB_HOST:-0.0.0.0}" \
    --port "${LANPROBE_WEB_PORT:-8080}" \
    --config-dir "$LANPROBE_DATA" &
HUB_PID=$!

# ── 5. Transmission des signaux + arrêt croisé ──────────────────────────────
shutting_down=false
term_handler() {
    $shutting_down && return
    shutting_down=true
    log "Signal d'arrêt reçu — arrêt propre du hub et d'InfluxDB."
    kill -TERM "$HUB_PID" 2>/dev/null || true
    kill -TERM "$INFLUXD_PID" 2>/dev/null || true
    wait "$HUB_PID" 2>/dev/null
    wait "$INFLUXD_PID" 2>/dev/null
    exit 0
}
trap term_handler TERM INT

log "Hub et InfluxDB démarrés (hub pid=${HUB_PID}, influxd pid=${INFLUXD_PID})."

# Un processus qui meurt sans qu'on l'ait demandé n'est pas laissé à moitié
# fonctionnel : on arrête l'autre et on remonte le même code de sortie.
while true; do
    if ! kill -0 "$HUB_PID" 2>/dev/null; then
        wait "$HUB_PID"; code=$?
        $shutting_down && exit 0
        log "Le hub s'est arrêté (code ${code}) — arrêt d'InfluxDB."
        kill -TERM "$INFLUXD_PID" 2>/dev/null || true
        wait "$INFLUXD_PID" 2>/dev/null || true
        exit "$code"
    fi
    if ! kill -0 "$INFLUXD_PID" 2>/dev/null; then
        wait "$INFLUXD_PID"; code=$?
        $shutting_down && exit 0
        log "InfluxDB s'est arrêté (code ${code}) — arrêt du hub."
        kill -TERM "$HUB_PID" 2>/dev/null || true
        wait "$HUB_PID" 2>/dev/null || true
        exit "$code"
    fi
    sleep 1
done
