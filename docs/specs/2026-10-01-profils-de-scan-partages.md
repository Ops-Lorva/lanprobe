# Profils de scan de ports partagés — spécification

**Demandé le 30/09/2026.** À relire avant toute ligne de code.

## Ce qui existe aujourd'hui

| Où | Quoi |
|---|---|
| Hub (interface) | **Quatre profils EN DUR** dans `web-ui/src/views/ProbeView.svelte` (`PORT_PROFILES` : `common`, `web`, `infra`, `db`). Ni base, ni API, ni écran d'édition. |
| Sonde | De vrais profils modifiables, dans sa configuration (`portscanProfiles`), utilisables **sans hub**. |
| App iOS | Une copie en dur des quatre du hub (`PortScanProfile`, ajoutée le 30/09). |
| Protocole | La sonde **remonte déjà sa configuration au hub à chaque battement**, profils compris (contrat § 16, implémenté). Le hub ne la **rend** qu'à l'enrôlement. |

🔴 **La sonde ne connaît aucun NOM de profil.** Elle reçoit une liste de ports
(`port_scan`, argument `ports`) ; l'interface décide lesquels. Rien dans cette
spec ne doit changer ça : lui faire connaître « web » obligerait à mettre à jour
toutes les sondes pour ajouter un profil.

## Décisions (Benjamin, 30/09)

1. **Le hub fait autorité** une fois la sonde enrôlée. Pas d'arbitrage « le plus
   récent gagne » : ce qu'il dit s'applique, création comme suppression.
2. **La suppression est écrite et datée**, jamais déduite d'une absence. Sans
   ça, une sonde qui ne l'a pas vue recrée le profil au battement suivant —
   c'est exactement la leçon de `removed_at` sur les surveillances.
3. Une sonde garde ses **profils de base** ; **ceux qu'elle crée en plus
   montent** et rejoignent la liste commune. La base retient **quelle sonde** a
   envoyé chaque profil.
4. **Portée : le hub entier.** Un profil de scan n'est pas privé — contrairement
   au profil RÉSEAU, qui décrit un site et reste sur la sonde.

## Modèle

```sql
CREATE TABLE portscan_profiles (
  profile_id   TEXT PRIMARY KEY,         -- stable, généré par qui crée
  name         TEXT NOT NULL,
  ports        TEXT NOT NULL,            -- JSON : [22, 80, 443]. Vide = la liste de la sonde
  origin_probe TEXT REFERENCES probes(probe_id),  -- NULL = créé sur le hub
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL,
  deleted_at   INTEGER                   -- non nul = supprimé, et c'est un FAIT daté
);
```

⚠️ `origin_probe` est une **trace**, pas un droit : elle ne donne aucune autorité
à la sonde qui a créé le profil. Elle sert à répondre « d'où sort celui-là ».

⚠️ `ports` vide **n'est pas** une liste vide envoyée à la sonde : c'est
« la sonde garde la sienne ». La sonde traite déjà `[]` comme une absence, et
confondre les deux ferait un scan complet là où on croyait restreindre.

## Routes

| Route | Auth | Rôle | Effet |
|---|---|---|---|
| `GET /api/portscan-profiles` | session | `viewer` | la liste, supprimés exclus |
| `POST /api/portscan-profiles` | session | `operator` | crée |
| `PATCH /api/portscan-profiles/{id}` | session | `operator` | renomme / change les ports |
| `DELETE /api/portscan-profiles/{id}` | session | `operator` | pose `deleted_at`, **ne supprime aucune ligne** |

## Descente vers les sondes

La réponse au battement gagne un champ `portscan_profiles`, **supprimés
compris**, exactement comme `monitors` :

```json
"portscan_profiles": [
  { "profile_id": "web", "name": "Web", "ports": [80,443], "deleted_at": null },
  { "profile_id": "x7", "name": "Caméras", "ports": [554], "deleted_at": 1790000000 }
]
```

🔴 **Les supprimés en font partie**, et c'est le cœur du mécanisme : sans eux, la
sonde ne distingue pas « le hub ne connaît pas ce profil » de « le hub l'a
supprimé », et le recrée en le remontant au battement suivant.

⚠️ Une sonde qui ne reçoit PAS le champ (hub plus ancien) ne touche à rien. Pas
de champ ≠ liste vide — sinon la première sonde à parler à un vieux hub perdrait
tous ses profils.

## Montée depuis les sondes

La configuration remontée (§ 16) contient déjà les profils. Le hub :
- **ignore** ceux qu'il connaît (le hub fait autorité, la sonde ne modifie rien) ;
- **ignore** ceux qu'il a supprimés (`deleted_at` non nul) — sinon la suppression
  se ferait annuler par la première sonde qui n'a pas encore battu ;
- **ingère** les inconnus, avec `origin_probe` = cette sonde.

## Ce que la sonde fait, à chaque battement

Benjamin, 30/09 : « si c'est la sonde qui contacte le hub, elle a juste à voir
ce que le hub a et se mettre à jour ». C'est exactement ça, et dans cet ordre :

1. elle **envoie** sa configuration, profils compris (déjà fait, § 16) ;
2. elle **lit** `portscan_profiles` dans la réponse ;
3. pour chaque profil reçu : `deleted_at` non nul → elle le retire de chez elle ;
   sinon → elle l'écrit tel quel (le hub fait autorité, il n'y a rien à
   arbitrer) ;
4. ce qu'elle a et que le hub ne mentionne pas est un profil **qu'elle vient de
   créer** : il est déjà parti à l'étape 1, le hub l'ingérera, et il reviendra
   au battement suivant. ⚠️ **Elle ne le supprime pas** au motif qu'il n'est pas
   dans la liste — sinon tout profil créé localement disparaîtrait une minute
   après sa création.

Délai d'un bout à l'autre : **un battement**, soit 60 s au réglage actuel du hub.

### 🔴 Pourquoi aucune DATE n'est comparée côté sonde

L'idée naturelle — « la sonde retient la date de sa dernière synchronisation et
regarde ce qui a changé depuis » — bute sur un fait du produit : **la sonde et le
hub ne partagent pas d'horloge**. C'est déjà la raison pour laquelle la file des
changements de surveillance voyage en **ancienneté** (`age_secs`) et non en
horodatage, et pourquoi la reprise après redémarrage préfère sous-estimer un âge
plutôt que d'utiliser une horloge murale, qui saute au réveil de veille et se
fait corriger par NTP.

Faire porter la suppression par la **liste elle-même** (`deleted_at` présent dans
la réponse) supprime la question : la sonde n'a aucune date à tenir, aucune
dérive à subir, et le même battement rejoué deux fois donne le même résultat.

## Migration

Les quatre profils en dur deviennent les quatre premières lignes, posées au
premier démarrage de cette version : `common` (ports vides), `web`, `infra`,
`db`. ⚠️ `infra` répète `161` dans le code actuel du hub — la ligne posée en base
est **triée et dédoublonnée**, sinon le nombre de ports annoncé à l'écran est
faux.

## Côté iOS

L'app lit `GET /api/portscan-profiles` et n'affiche plus sa copie en dur.
⚠️ Elle la **garde en repli** : un hub antérieur à cette version ne sert pas la
route, et un écran sans aucun profil serait pire que quatre profils connus.

## Ce que cette spec ne fait PAS

- pas de portée par site (décision 4) ;
- pas d'autorité à la sonde : une modification locale d'un profil du hub est
  écrasée au battement suivant, et l'interface de la sonde doit le dire plutôt
  que de laisser croire à une édition qui tiendra ;
- aucune suppression de ligne en base, jamais.
