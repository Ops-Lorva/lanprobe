# LanProbe Web — contrat d'interface

Document de référence partagé entre le **hub** (`crates/lanprobe-web`, auto-hébergé
en conteneur) et la **sonde** (`crates/lanprobe-server` + client desktop Tauri).

Toute divergence entre les deux implémentations se règle ici, pas dans le code.

---

## 1. Vue d'ensemble

```
┌────────────────┐   1. enrôlement (HTTPS, compte admin)   ┌──────────────────┐
│  sonde         │ ─────────────────────────────────────▶  │  lanprobe-web    │
│  (desktop ou   │ ◀───────  jeton + coordonnées Influx ──  │  (conteneur)     │
│   headless)    │                                          │                  │
│                │   2. battement de cœur (jeton sonde)     │  SQLite : users, │
│                │ ─────────────────────────────────────▶  │  probes          │
│                │                                          │                  │
│                │   3. mesures — écriture DIRECTE          │  InfluxDB 2      │
│                │ ─────────────────────────────────────────────▶ (embarqué)   │
└────────────────┘        line protocol, port dédié         └──────────────────┘
```

**Pourquoi trois flux et pas un seul.** Les mesures ne transitent pas par le hub :
elles vont droit dans Influx, qui est fait pour ça. Mais une sonde éteinte
n'écrit rien — et « rien » ressemble exactement à « tout va bien » dans une base
de séries temporelles. Le battement de cœur est ce qui distingue une sonde
silencieuse d'une sonde absente. Sans lui, l'interface afficherait un parc vert
pendant qu'une sonde est débranchée depuis trois jours.

## 2. Identité d'une sonde

| Champ | Rôle | Modifiable |
|---|---|---|
| `probe_id` | identité technique, UUIDv4, généré par le hub | non |
| `name` | étiquette lisible, saisie par l'utilisateur | oui |
| `token` | authentifie la sonde auprès du hub | rotation possible |

Le `name` est **une étiquette, pas une identité**. Le renommer ne casse pas
l'historique : les points Influx sont tagués `probe_id`, et le hub résout le nom
à l'affichage. Un parc où l'on ne peut pas renommer une sonde sans perdre ses
courbes est un parc que personne ne renomme.

Le tag Influx `host` reste écrit pour compatibilité avec les tableaux de bord
existants, mais `probe_id` est la clé de jointure.

## 3. Endpoints du hub

Toutes les réponses d'erreur : `{ "error": "<message lisible>" }` avec le code HTTP adéquat.

### `GET /api/status` — public
```json
{ "needs_setup": true, "version": "1.2.0" }
```
Seule route accessible sans authentification quand `needs_setup` est vrai.

### `POST /api/setup` — création de l'admin initial
```json
{ "setup_token": "...", "username": "admin", "password": "..." }
```
Le `setup_token` est généré au démarrage tant qu'aucun compte n'existe, écrit en
`0600` dans le volume et affiché dans les logs du conteneur (`docker logs`).
Non rejouable : consommé au premier succès.

### `POST /api/login` → cookie de session `HttpOnly`
### `POST /api/logout`

### `POST /api/probes/enroll` — authentification : identifiants du compte
```json
{ "username": "admin", "password": "...", "name": "Baie RDC" }
```
→ `201`
```json
{
  "probe_id": "0c1e…",
  "name": "Baie RDC",
  "token": "…",
  "influx": {
    "url": "https://lanprobe-web.example.org:8086",
    "org": "lanprobe",
    "bucket": "lanprobe",
    "token": "…"
  },
  "heartbeat_interval_secs": 60
}
```

Le jeton Influx renvoyé est **restreint en écriture au seul bucket**. Une sonde
compromise ne peut ni lire les mesures des autres, ni les effacer.

Si `name` est déjà pris, le hub répond `409` — deux sondes nommées « Bureau »
rendent l'interface inutilisable, autant le refuser tout de suite.

### `POST /api/probes/{id}/heartbeat` — authentification : `Authorization: Bearer <token sonde>`
```json
{
  "version": "1.2.0",
  "platform": "linux",
  "buffered_points": 0,
  "last_write_ok": true
}
```
→ `200 { "ok": true }`

`buffered_points` remonte la profondeur du tampon local : une sonde qui accumule
sans écrire est visible dans l'interface **avant** qu'on découvre le trou dans les
graphes.

### `GET /api/probes` — authentification : session
```json
[{
  "probe_id": "0c1e…", "name": "Baie RDC", "platform": "linux",
  "version": "1.2.0", "last_seen": 1756339200, "status": "online",
  "buffered_points": 0, "created_at": 1756332000
}]
```

`status` est **dérivé**, jamais stocké :
- `online` — dernier battement < 3 × `heartbeat_interval_secs`
- `stale` — < 24 h
- `offline` — au-delà

Stocker un statut obligerait quelqu'un à le mettre à jour ; une valeur dérivée ne
peut pas mentir sur sa propre fraîcheur.

### `PATCH /api/probes/{id}` — `{ "name": "Nouveau nom" }`
### `DELETE /api/probes/{id}` — révoque le jeton. **Ne supprime aucune mesure.**
### `GET /api/probes/{id}/metrics?measurement=&range=` — requête Flux proxifiée

Le navigateur ne parle jamais directement à Influx : le jeton de lecture reste
côté serveur.

## 4. Schéma SQLite

```sql
CREATE TABLE users (
  id            INTEGER PRIMARY KEY,
  username      TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,          -- argon2id
  role          TEXT NOT NULL DEFAULT 'admin',
  created_at    INTEGER NOT NULL
);

CREATE TABLE probes (
  probe_id        TEXT PRIMARY KEY,
  name            TEXT NOT NULL UNIQUE,
  token_hash      TEXT NOT NULL,        -- argon2id : jamais le jeton en clair
  platform        TEXT,
  version         TEXT,
  last_seen       INTEGER,
  buffered_points INTEGER DEFAULT 0,
  created_at      INTEGER NOT NULL,
  revoked_at      INTEGER
);
```

Le jeton de sonde est haché comme un mot de passe. Une base volée ne donne pas
le droit d'écrire au nom des sondes.

## 5. Tampon hors ligne (côté sonde)

**Défaut à corriger d'abord** — `influxdb.rs` vide le tampon *avant* l'écriture :

```rust
let body = buffer.join("\n");
buffer.clear();                  // ← perte définitive si l'écriture échoue
if let Err(e) = client.write(body).await { tracing::warn!(…) }
```

Comportement attendu :

1. **Ne pas vider avant confirmation.** En cas d'échec, les points retournent en
   tête de file et le prochain tick réessaie.
2. **Persistance disque** — `buffer.ndjson` dans le répertoire de config, relu au
   démarrage. Une coupure de courant pendant l'incident ne doit pas effacer la
   trace de l'incident.
3. **Borné explicitement** — `MAX_BUFFER = 100_000` points *et* 24 h d'âge. Au
   débordement, on jette **les plus anciens** et on **journalise le nombre exact**.
   Un tampon qui déborde en silence ment sur la complétude des données.
4. **Horodatage préservé** — les points sont déjà datés à la mesure dans
   `event_to_points`. Ne pas re-dater à l'envoi : le rejeu doit retomber au bon
   endroit dans le graphe.
5. **Repli exponentiel** plafonné à 60 s entre deux tentatives, pour ne pas
   marteler un Influx qui redémarre.

## 6. Ce que la sonde stocke

Dans `app_config.json`, section `hub`, chiffrée par le scellement existant
(`secrets.rs`, marqueur `enc:v1:`) :

```json
{ "url": "https://…", "probe_id": "…", "name": "…", "token": "enc:v1:…",
  "influx": { "url": "…", "org": "…", "bucket": "…", "token": "enc:v1:…" } }
```

Le mot de passe du compte n'est **jamais** écrit sur disque : il sert une fois,
à l'enrôlement.
