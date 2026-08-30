#!/usr/bin/env bash
# entrypoint.sh — point d'entrée du conteneur, exécuté en root (PID 2, enfant
# de tini). Son seul rôle : préparer les volumes puis abandonner les
# privilèges pour de bon. Toute la logique applicative (initialisation
# InfluxDB, démarrage des deux processus) vit dans run.sh, exécuté ensuite en
# tant qu'utilisateur non-root.
#
# Pourquoi une phase root du tout : un volume Docker nommé fraîchement créé
# appartient à root:root, quel que soit l'utilisateur déclaré dans l'image.
# Sans cette étape, InfluxDB et le hub ne pourraient pas écrire dans leurs
# propres volumes au premier démarrage. C'est l'unique exception au principe
# de moindre privilège de cette image, et elle est aussi courte que possible :
# aucune commande applicative ne s'exécute en root, seul `chown` est appelé
# ici, puis `exec` remplace immédiatement ce processus par l'utilisateur final
# (pas de démon root résiduel).
set -euo pipefail

LANPROBE_UID=10001
LANPROBE_GID=10001

# /backup en fait partie : le hub y écrit ses archives, en utilisateur non
# privilégié. Sans ce chown, un volume nommé neuf reste à root:root et la
# sauvegarde quotidienne échoue — silencieusement du point de vue de
# l'utilisateur, qui découvrirait le vide le jour de la restauration.
DATA_DIRS=(/data/influxdb "${LANPROBE_WEB_BACKUP_DIR:-/backup}")

for dir in "${DATA_DIRS[@]}"; do
    mkdir -p "$dir"
    owner="$(stat -c '%u:%g' "$dir")"
    if [ "$owner" != "${LANPROBE_UID}:${LANPROBE_GID}" ]; then
        # `chown -R` seulement si nécessaire : sur un volume InfluxDB qui a
        # grossi de plusieurs Go après des mois d'usage, un chown récursif
        # inconditionnel à chaque redémarrage serait inutilement coûteux.
        chown -R "${LANPROBE_UID}:${LANPROBE_GID}" "$dir"
    fi
done

# 🔴 Le volume du hub, lui, est repris À CHAQUE DÉMARRAGE, sans regarder le
# propriétaire du répertoire.
#
# Il ne pèse que quelques mégaoctets — base SQLite, certificat, clés — donc
# l'argument de coût ci-dessus ne s'y applique pas. Et surtout, le test sur le
# seul répertoire ne voit pas le cas qui casse : `lanprobe-web restore`
# s'exécute en `docker exec`, donc en **root**, et repose une base et des
# secrets appartenant à root DANS un répertoire qui, lui, appartient toujours
# à `lanprobe`. Le chown conditionnel était alors sauté, et le hub redémarrait
# sur « unable to open database file » — c'est-à-dire précisément le jour de
# la restauration, le seul jour où l'on compte dessus.
mkdir -p /data/lanprobe
chown -R "${LANPROBE_UID}:${LANPROBE_GID}" /data/lanprobe

# À partir d'ici, plus jamais root : gosu fait un execve() direct vers
# l'utilisateur applicatif (ce n'est pas un sudo, il ne reste aucun processus
# root en arrière-plan).
exec gosu "${LANPROBE_UID}:${LANPROBE_GID}" /usr/local/bin/run.sh "$@"
