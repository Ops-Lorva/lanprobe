#!/usr/bin/env bash
# healthcheck.sh — vérifie que le hub RÉPOND, pas que le processus existe.
#
# GET /api/status est la seule route publique du contrat (voir
# docs/lanprobe-web-contrat.md §3) : elle répond 200 avec un champ
# "needs_setup" que l'instance ait ou non déjà un compte admin. On vérifie
# donc le code HTTP *et* le contenu — un 200 renvoyé par autre chose que le
# hub (mauvaise route, proxy cassé) ne doit pas passer pour "en bonne santé".
set -euo pipefail

PORT="${LANPROBE_WEB_PORT:-8080}"
# Le hub sert en clair par défaut, derrière le reverse proxy de l'utilisateur ;
# --tls (LANPROBE_WEB_TLS) le fait terminer le TLS lui-même. Le healthcheck
# doit suivre, sinon il déclarerait le conteneur malade alors qu'il répond.
case "${LANPROBE_WEB_TLS:-false}" in
    true|1|yes) SCHEME=https ;;
    *)          SCHEME=http  ;;
esac

response="$(curl -fsSk --max-time 3 "${SCHEME}://127.0.0.1:${PORT}/api/status")" || exit 1

case "$response" in
    *'"needs_setup"'*) exit 0 ;;
    *) exit 1 ;;
esac
