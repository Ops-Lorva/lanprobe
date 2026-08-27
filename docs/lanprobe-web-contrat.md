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

## 2. Sites et identité d'une sonde

### Sites

Les sondes sont groupées par **site**, sur le modèle Ubiquiti : un seul niveau de
regroupement, pas de hiérarchie client → site.

En pratique le site correspond au client, et les sondes portent les lieux : un
site « Durand » contenant les sondes « Paris », « Lyon », « Entrepôt ». Rien ne
l'impose — le nom du site est libre — mais c'est la lecture qui marche quand on
gère plusieurs clients depuis un seul hub.

**Un seul bucket InfluxDB pour tous les sites**, le site étant un *tag* sur les
points. Un bucket Influx porte avant tout une politique de rétention — ce n'est
pas une frontière de sécurité exploitable ici, puisque les jetons de sonde sont
en **écriture seule** et que le jeton de lecture ne quitte jamais le hub. Le
contrôle d'accès est fait par le hub, en SQL, pas par le découpage Influx.

Découper en un bucket par site imposerait un `union()` sur N buckets dès qu'on
veut une vue « tous les sites », et rendrait les variables Grafana pénibles. La
seule raison légitime d'y revenir serait une **rétention différente par site** —
on ajouterait alors un bucket dédié à ce site sans rien casser. L'inverse
(partir en buckets et vouloir revenir) serait une migration de données.

### Identité d'une sonde

| Champ | Rôle | Modifiable |
|---|---|---|
| `probe_id` | identité technique, UUIDv4, généré par le hub | non |
| `name` | étiquette lisible, saisie par l'utilisateur | oui |
| `site_id` | site d'appartenance | oui (déplacement) |
| `token` | authentifie la sonde auprès du hub | rotation possible |

Le `name` est **une étiquette, pas une identité**. Le renommer ne casse pas
l'historique : les points Influx sont tagués `probe_id`, et le hub résout le nom
à l'affichage. Un parc où l'on ne peut pas renommer une sonde sans perdre ses
courbes est un parc que personne ne renomme.

Le nom d'une sonde est unique **au sein d'un site**, pas globalement : deux sites
ont chacun le droit d'avoir leur « Baie RDC ».

### Tags écrits sur les points

| Tag | Stabilité | À quoi il sert |
|---|---|---|
| `probe_id` | stable | clé de jointure — ne change jamais |
| `site_id` | stable | filtrage par site, résiste au renommage |
| `probe` | change au renommage | lisibilité directe dans Grafana |
| `site` | change au renommage | lisibilité directe dans Grafana |
| `host` | — | conservé pour les tableaux de bord existants |

On écrit **à la fois l'identifiant et le nom**, en connaissance de cause : un
renommage crée une nouvelle série pour les tags lisibles, et les points antérieurs
gardent l'ancien nom. Le coût en cardinalité est négligeable (quelques renommages
sur la vie d'un parc) et le bénéfice est réel : les jointures restent stables sur
les identifiants, et quelqu'un qui branche Grafana directement sur le bucket lit
des noms plutôt que des UUID. Interroger toujours sur `probe_id` / `site_id`
quand on veut l'historique complet d'une sonde.

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

### `GET /api/sites` — authentification : session **ou** identifiants du compte
La sonde l'appelle avant l'enrôlement pour proposer une liste déroulante plutôt
qu'un champ libre — c'est ce qui évite qu'un « Durnad » se glisse dans le parc.
```json
[{ "site_id": "…", "name": "Durand", "probe_count": 3 }]
```

### `POST /api/sites` — `{ "name": "Durand" }` → `201`. `409` si le nom existe déjà.
### `PATCH /api/sites/{id}` — renommage. Les mesures passées gardent l'ancien tag `site` mais restent jointes par `site_id`.

### `POST /api/probes/enroll` — authentification : identifiants du compte
```json
{ "username": "admin", "password": "...", "name": "Paris", "site": "Durand" }
```
`site` est résolu par nom, insensible à la casse. S'il n'existe pas, le hub le
crée et le signale par `site_created: true` — le champ existe pour que l'app
affiche « nouveau site créé », faute de quoi une faute de frappe passerait
inaperçue jusqu'à ce qu'on cherche une sonde dans le mauvais parc.

→ `201`
```json
{
  "probe_id": "0c1e…",
  "name": "Paris",
  "site_id": "…",
  "site": "Durand",
  "site_created": false,
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

Si `name` est déjà pris **dans ce site**, le hub répond `409` — deux sondes
« Paris » chez Durand rendent l'interface inutilisable, autant le refuser tout de
suite. Le même nom dans un autre site est parfaitement légitime.

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

### `GET /api/probes[?site=<site_id>]` — authentification : session
```json
[{
  "probe_id": "0c1e…", "name": "Paris",
  "site_id": "…", "site": "Durand", "platform": "linux",
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

### `PATCH /api/probes/{id}` — `{ "name": "Nouveau nom", "site_id": "…" }`
Les deux champs sont facultatifs. Changer `site_id` déplace la sonde d'un site à
l'autre ; son historique la suit, puisqu'il est joint sur `probe_id`.
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

CREATE TABLE sites (
  site_id    TEXT PRIMARY KEY,          -- UUIDv4
  name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
  created_at INTEGER NOT NULL
);

CREATE TABLE probes (
  probe_id        TEXT PRIMARY KEY,
  site_id         TEXT NOT NULL REFERENCES sites(site_id),
  name            TEXT NOT NULL,
  token_hash      TEXT NOT NULL,        -- argon2id : jamais le jeton en clair
  platform        TEXT,
  version         TEXT,
  last_seen       INTEGER,
  buffered_points INTEGER DEFAULT 0,
  created_at      INTEGER NOT NULL,
  revoked_at      INTEGER
);

-- Le nom est unique DANS un site, pas globalement.
CREATE UNIQUE INDEX probes_site_name ON probes(site_id, name COLLATE NOCASE);
```

`COLLATE NOCASE` sur les noms : « Durand » et « durand » sont le même site.
Sans ça, un doublon de casse crée deux parcs distincts et personne ne comprend
pourquoi la moitié des sondes a disparu.

Supprimer un site n'est pas prévu tant qu'il porte des sondes — le hub répond
`409`. Aucune suppression en cascade dans ce projet.

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
{ "url": "https://…", "probe_id": "…", "name": "…",
  "site_id": "…", "site": "…", "token": "enc:v1:…",
  "influx": { "url": "…", "org": "…", "bucket": "…", "token": "enc:v1:…" } }
```

La sonde a besoin de `site_id` et `site` en local parce que **c'est elle qui
écrit les tags** sur ses points. Après un renommage de site côté hub, elle
récupère la nouvelle valeur dans la réponse au battement de cœur :

```json
{ "ok": true, "name": "Paris", "site_id": "…", "site": "Durand" }
```

Sans ce retour, une sonde continuerait d'écrire l'ancien nom indéfiniment après
un renommage — et le tag lisible divergerait de l'interface, ce qui est pire que
de ne pas l'avoir.

Le mot de passe du compte n'est **jamais** écrit sur disque : il sert une fois,
à l'enrôlement.

## 7. Configuration du hub (contrat avec le packaging)

Le binaire `lanprobe-web` accepte les mêmes options que `lanprobe-server` :
`--host`, `--port`, `--config-dir`. Variables d'environnement :

| Variable | Rôle |
|---|---|
| `INFLUX_TOKEN` | jeton opérateur. **Ne sort jamais du hub.** |
| `INFLUX_URL` | URL interne d'Influx, vue du conteneur (`https://127.0.0.1:8086`) |
| `INFLUX_ORG`, `INFLUX_BUCKET` | défauts `lanprobe` / `lanprobe` |
| `INFLUX_ADVERTISE_URL` | URL d'Influx **renvoyée aux sondes** à l'enrôlement |
| `LANPROBE_WEB_HOST`, `LANPROBE_WEB_PORT`, `LANPROBE_WEB_CONFIG_DIR` | défauts `0.0.0.0`, `8443`, `/data/lanprobe` |

### L'URL annoncée n'est pas l'URL interne

Le hub joint Influx sur `127.0.0.1` ; une sonde sur le LAN, non. Renvoyer l'URL
interne à l'enrôlement produirait une sonde qui s'enrôle avec succès puis
n'écrit jamais rien — la panne la plus pénible à diagnostiquer, parce que tout
paraît avoir marché.

**Ordre de résolution de l'URL annoncée :**
1. `INFLUX_ADVERTISE_URL` si définie ;
2. sinon, l'hôte de l'en-tête `Host` de la requête d'enrôlement, avec le port
   Influx. La sonde vient de joindre le hub à cette adresse — Influx est sur la
   même machine, c'est le meilleur pari disponible ;
3. journaliser laquelle des deux a été retenue.

Le repli sur l'en-tête `Host` évite d'imposer une variable obligatoire à qui
monte le conteneur : dans le cas courant, ça marche sans rien configurer.

### TLS auto-signé côté Influx

Influx est servi en HTTPS avec un certificat auto-signé généré au premier
démarrage. La sonde doit donc pouvoir **épingler ou accepter explicitement** ce
certificat. La réponse d'enrôlement inclut son empreinte :

```json
"influx": { "url": "…", "org": "…", "bucket": "…", "token": "…",
            "tls_fingerprint_sha256": "AB:CD:…" }
```

Épingler l'empreinte vaut mieux que désactiver la vérification : on garde
l'authentification du serveur sans exiger une autorité de certification que
personne n'a dans un réseau local.
