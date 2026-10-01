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

## Alléger le battement : une révision, pas une liste

Objection de Benjamin (30/09) : « on ne va pas faire voyager 40 profils
supprimés à chaque fois ». Exact — et une pierre tombale qui vit pour toujours
est une fuite lente.

Le hub tient un **compteur de révision**, incrémenté à chaque création,
modification ou suppression de profil. Chaque ligne porte la révision qui l'a
produite.

```sql
ALTER TABLE portscan_profiles ADD COLUMN rev INTEGER NOT NULL;   -- révision de ce changement
-- + une ligne par sonde : last_profiles_rev, écrite quand elle accuse
```

Au battement, la sonde envoie la dernière révision qu'elle a appliquée
(`profiles_rev`) ; le hub répond :

| Cas | Réponse |
|---|---|
| à jour | **rien** — c'est le cas courant, et il ne coûte rien |
| en retard, historique encore là | **le delta** : seulement les lignes de révision supérieure, pierres tombales comprises |
| révision inconnue ou trop ancienne | **la liste complète**, avec un drapeau « remplace tout » |

🔴 **Le numéro de révision n'est PAS une date.** Il vient du hub, la sonde le
range et le rend tel quel : aucune horloge n'entre dans l'affaire, et deux
machines qui ne seront jamais d'accord sur l'heure n'ont rien à arbitrer.

### Quand une pierre tombale peut partir

Le hub connaît la révision de **chaque** sonde : il peut donc effacer pour de
bon une suppression dont la révision est inférieure à la **plus petite révision
du parc**. Plus personne n'a besoin de l'apprendre, elle n'a plus rien à dire.

⚠️ Les sondes **révoquées ou archivées** ne comptent pas dans ce minimum : une
machine partie à la benne bloquerait le nettoyage pour toujours.

⚠️ Une sonde simplement éteinte depuis des mois, elle, compte — et c'est voulu :
le jour où elle revient, elle doit apprendre les suppressions. Garde-fou : une
révision plus vieille que **90 jours** cesse de retenir le nettoyage, et la
sonde qui revient après ça reçoit la **liste complète** plutôt qu'un delta. Elle
repart juste.

### Le compteur peut-il déborder ?

Non. `INTEGER` en SQLite est un entier signé 64 bits : à **un changement par
seconde pendant un siècle**, on aurait consommé trois milliardièmes de la
plage. Aucun rebouclage à prévoir, et surtout **aucune remise à zéro à écrire** :
c'est elle qui serait dangereuse.

🔴 **Le vrai risque n'est pas le débordement, c'est le RECUL.** Une base
restaurée depuis une sauvegarde revient à une révision plus ancienne, alors que
les sondes, elles, ont gardé la leur. Une sonde qui annonce 42 à un hub revenu à
38 n'aurait plus jamais rien à apprendre : le hub lui répondrait « tu es à jour »
pour toujours, et sa liste de profils gèlerait sans que rien ne le dise.

Deux gardes, et elles coûtent trois lignes :

1. **au démarrage**, le compteur du hub est remis à `max(compteur, plus grande
   révision présente dans la table)` — une restauration partielle ne peut pas le
   faire reculer sous ses propres lignes ;
2. **une sonde qui annonce une révision SUPÉRIEURE à celle du hub est traitée
   comme inconnue** : elle reçoit la liste complète avec le drapeau « remplace
   tout ». C'est exactement le cas « hub restauré », et il se répare tout seul au
   premier battement.

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
