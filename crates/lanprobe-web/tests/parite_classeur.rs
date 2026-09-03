//! Parité des deux générateurs de classeur SLA — côté hub.
//!
//! 🔴 **Ce fichier et `web-ui/src/lib/sla-report-parity.test.ts` forment une
//! seule épreuve.** Une charge utile figée (`tests/parite-classeur/charge-*.
//! json`) est jouée par les deux générateurs. Le test du navigateur écrit son
//! relevé canonique (`releve-navigateur.json`) ; celui-ci produit le sien et
//! les compare **cellule par cellule**.
//!
//! Pourquoi cette épreuve existe : le navigateur alimente les clients
//! aujourd'hui, le hub prendra la suite. Si les deux divergent, personne ne le
//! verra — les deux rendent un document crédible, et un client recevrait un
//! chiffre différent du même mois selon l'outil qui a produit le fichier.
//!
//! ⚠️ **Chaque écart est énuméré et QUALIFIÉ**, un par un, dans
//! `ecarts-attendus.json` : `date`, `graphique`, `largeur`, `cellule-vide` ou
//! `defaut` (voir [`NATURES`]). Le test échoue sur un écart **nouveau** comme
//! sur un écart **disparu** : dans les deux cas la bascule mérite une décision,
//! pas un silence.
//!
//! ⚠️ **La parité des IMAGES n'est pas établie ici** et ne peut pas l'être :
//! `vitest` tourne sans `<canvas>`, donc le classeur du navigateur n'a aucun
//! graphique dans ce test alors qu'il en a chez l'utilisateur.
//!
//! ⚠️ Les trois modules du hub sont recompilés dans ce binaire de test
//! (`#[path]`) parce que `report_xlsx::build` est `pub(crate)` : une épreuve
//! d'intégration ne l'atteint pas autrement. C'est bien le MÊME code source,
//! pas une copie — un changement dans `src/` change ce test.
//!
//! 🔴 **Ne jamais lancer `rustfmt` sur CE fichier.** `rustfmt` suit les
//! `#[path]` et reformate les trois modules de `src/` au passage : on croit
//! ranger un test, on réécrit 95 lignes de code de production dans le même
//! commit. Constaté le 03/09/2026 et annulé. Formater le crate entier
//! (`cargo fmt -p lanprobe-web`) si le besoin se présente — pas ce fichier
//! seul.

#[path = "../src/report_i18n.rs"]
mod report_i18n;

#[path = "../src/report_chart.rs"]
mod report_chart;

#[path = "../src/report_xlsx.rs"]
mod report_xlsx;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

/// Où vivent la charge utile figée et les relevés.
fn racine() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parite-classeur")
}

/// Les trois scénarios joués. Ils ne se recouvrent pas :
/// - `solo` : une sonde, les six familles de feuilles ;
/// - `multi` : deux sondes, colonne « Sonde » et collision de noms d'onglets ;
/// - `noms` : troncature à 31 caractères, caractères refusés par Excel,
///   doublons après troncature, et fenêtre relative (`-7d`, pluriel ICU).
const SCENARIOS: [&str; 3] = ["solo", "multi", "noms"];

// ── Ce que les deux côtés écrivent ───────────────────────────────────────

/// Une cellule telle qu'un tableur la relira.
///
/// 🔴 `Texte` et `Nombre` ne sont pas interchangeables : un tableur agrège ce
/// qui est numérique. C'est précisément ce que la comparaison doit attraper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum Cellule {
    Nombre { n: f64 },
    Texte { s: String },
}

impl std::fmt::Display for Cellule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cellule::Texte { s } => write!(f, "texte {s:?}"),
            Cellule::Nombre { n } => write!(f, "nombre {n}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FeuilleRelevee {
    nom: String,
    /// Indexées par référence Excel (« A1 »). Une cellule **absente** du
    /// fichier est absente d'ici ; une cellule à chaîne vide y figure, avec sa
    /// chaîne vide. La nuance est l'un des écarts relevés.
    cellules: BTreeMap<String, Cellule>,
    /// Largeur de chaque colonne, en caractères.
    largeurs: Vec<f64>,
    /// Nombre de dessins ancrés sur la feuille.
    images: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Releve {
    scenarios: BTreeMap<String, Vec<FeuilleRelevee>>,
}

/// Un écart constaté, dit en toutes lettres.
///
/// ⚠️ La forme est faite pour être LUE : c'est la liste qu'on présente avant
/// d'autoriser la bascule, pas un identifiant de test.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Ecart {
    scenario: String,
    feuille: String,
    /// Référence de cellule (« B4 »), largeur de colonne, ou dessin ancré.
    ou: String,
    navigateur: String,
    hub: String,
}

/// Un écart **qualifié**. La qualification est faite à la main, une fois, et
/// relue : c'est elle qui distingue « on a décidé » de « on n'a pas vu ».
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EcartAttendu {
    nature: String,
    #[serde(flatten)]
    ecart: Ecart,
}

/// Les seules qualifications admises.
///
/// - `date` : le hub localise et nomme son fuseau, le navigateur passe par
///   `Intl` dans le fuseau du poste. **Tranché par Benjamin**, documenté dans
///   `report_i18n.rs`.
/// - `graphique` : le classeur du navigateur n'a pas d'image sous Node, faute
///   de `<canvas>`. **La parité des images n'est pas prouvée**, ni ici ni
///   ailleurs.
/// - `largeur` : largeur de colonne. `rust_xlsxwriter` ajoute la marge de
///   0,71 caractère qu'Excel retranche à l'affichage ; les colonnes de dates
///   s'élargissent parce que la date du hub est plus longue. Cosmétique.
/// - `cellule-vide` : le navigateur pose une cellule à chaîne VIDE (`addRow`
///   avec `''`) là où le hub n'écrit rien. Invisible à l'œil, **pas** à
///   `ISBLANK` ni à `NB()` : un client qui compte une colonne n'obtient pas le
///   même nombre. Écart réel, à trancher — mais aucun chiffre n'est faux.
/// - `defaut` : 🔴 **le hub et le navigateur n'écrivent pas le même chiffre.**
///   Un test dédié refuse d'en laisser passer un seul — voir
///   [`aucun_ecart_nest_un_defaut_de_chiffre`].
const NATURES: [&str; 5] = ["date", "graphique", "largeur", "cellule-vide", "defaut"];

// ── La charge utile figée ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Charge {
    locale: String,
    site_name: String,
    range_start: i64,
    range_stop: i64,
    payloads: Vec<serde_json::Value>,
}

// ── Relire le classeur produit par le hub ────────────────────────────────
//
// ⚠️ On rouvre le FICHIER, jamais les structures qui ont servi à l'écrire : un
// générateur qui se vérifie sur son propre état interne ne prouve rien du
// document remis au client. Un XLSX est un zip de XML, et `zip` est déjà une
// dépendance du hub.

fn attribut(s: &str, nom: &str) -> Option<String> {
    let motif = format!("{nom}=\"");
    let i = s.find(&motif)? + motif.len();
    let reste = &s[i..];
    let j = reste.find('"')?;
    Some(desechappe(&reste[..j]))
}

fn desechappe(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn relever_hub(bytes: &[u8]) -> Vec<FeuilleRelevee> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))
        .expect("le classeur du hub doit être un zip lisible");
    let lire = |zip: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, nom: &str| -> String {
        let mut f = zip
            .by_name(nom)
            .unwrap_or_else(|_| panic!("{nom} absent du classeur"));
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        s
    };

    let wb = lire(&mut zip, "xl/workbook.xml");
    let onglets: Vec<String> = wb
        .split("<sheet ")
        .skip(1)
        .filter_map(|c| attribut(c, "name"))
        .collect();

    // Chaînes partagées : une cellule `t="s"` porte un index, pas un texte.
    let partagees: Vec<String> = match zip.by_name("xl/sharedStrings.xml") {
        Ok(mut f) => {
            let mut s = String::new();
            f.read_to_string(&mut s).unwrap();
            s.split("<si>")
                .skip(1)
                .map(|bloc| {
                    let bloc = &bloc[..bloc.find("</si>").unwrap_or(bloc.len())];
                    let mut texte = String::new();
                    for morceau in bloc.split("<t").skip(1) {
                        let debut = morceau.find('>').unwrap() + 1;
                        let fin = morceau.find("</t>").unwrap_or(morceau.len());
                        texte.push_str(&desechappe(&morceau[debut..fin]));
                    }
                    texte
                })
                .collect()
        }
        Err(_) => Vec::new(),
    };

    let mut out = Vec::new();
    for (i, nom) in onglets.iter().enumerate() {
        let xml = lire(&mut zip, &format!("xl/worksheets/sheet{}.xml", i + 1));

        let mut cellules = BTreeMap::new();
        for chunk in xml.split("<c ").skip(1) {
            let fin = chunk.find("</c>").unwrap_or(chunk.len());
            let chunk = &chunk[..fin];
            let Some(r) = attribut(chunk, "r") else {
                continue;
            };
            let t = attribut(chunk, "t");
            let Some(vdeb) = chunk.find("<v>") else {
                continue;
            };
            let vfin = chunk[vdeb..].find("</v>").unwrap() + vdeb;
            let brut = &chunk[vdeb + 3..vfin];
            let valeur = match t.as_deref() {
                Some("s") => Cellule::Texte {
                    s: partagees[brut.parse::<usize>().unwrap()].clone(),
                },
                Some("str") | Some("inlineStr") => Cellule::Texte {
                    s: desechappe(brut),
                },
                _ => Cellule::Nombre {
                    n: brut.parse().unwrap(),
                },
            };
            // ⚠️ Une chaîne VIDE se relève comme telle, pas comme « rien ».
            // `exceljs` écrit une cellule pour `''` là où le hub n'en écrit
            // aucune ; `ISBLANK` et `NB()` les distinguent. Les confondre ici
            // ferait disparaître un écart réel du rapport.
            cellules.insert(r, valeur);
        }

        // Largeurs : `<col min="1" max="1" width="14" .../>`.
        let mut largeurs: Vec<f64> = Vec::new();
        if let Some(deb) = xml.find("<cols>") {
            let bloc = &xml[deb..xml[deb..]
                .find("</cols>")
                .map(|j| deb + j)
                .unwrap_or(xml.len())];
            for col in bloc.split("<col ").skip(1) {
                let min: usize = attribut(col, "min")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
                let max: usize = attribut(col, "max")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(min);
                let w: f64 = attribut(col, "width")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.0);
                if largeurs.len() < max {
                    largeurs.resize(max, 0.0);
                }
                for c in min..=max {
                    largeurs[c - 1] = (w * 100.0).round() / 100.0;
                }
            }
        }

        out.push(FeuilleRelevee {
            nom: nom.clone(),
            cellules,
            largeurs,
            // Un dessin ancré = le graphique. Le compter, plutôt que le
            // comparer : sous Node le navigateur n'en produit aucun.
            images: usize::from(xml.contains("<drawing ")),
        });
    }
    out
}

// ── L'épreuve ────────────────────────────────────────────────────────────

fn construire(charge: &Charge) -> Vec<FeuilleRelevee> {
    let workbook = report_xlsx::Workbook {
        site_name: charge.site_name.clone(),
        range_start: charge.range_start,
        range_stop: charge.range_stop,
        payloads: charge.payloads.clone(),
    };
    let catalog = report_i18n::Catalog::load(&charge.locale);
    let built = report_xlsx::build(&workbook, &catalog).expect("le hub doit produire le classeur");
    relever_hub(&built.bytes)
}

fn charge(nom: &str) -> Charge {
    let chemin = racine().join(format!("charge-{nom}.json"));
    let brut = std::fs::read_to_string(&chemin)
        .unwrap_or_else(|e| panic!("charge utile {} illisible : {e}", chemin.display()));
    serde_json::from_str(&brut).expect("charge utile mal formée")
}

fn releve_navigateur() -> Releve {
    let chemin = racine().join("releve-navigateur.json");
    let brut = std::fs::read_to_string(&chemin).unwrap_or_else(|e| {
        panic!(
            "relevé du navigateur absent ({}) : lancer `PARITE_CLASSEUR=maj npm run test:web` — {e}",
            chemin.display()
        )
    });
    serde_json::from_str(&brut).expect("relevé du navigateur mal formé")
}

/// Compare deux relevés d'un même scénario et rend la liste des écarts.
///
/// ⚠️ La structure (nombre, nom et ordre des onglets) n'entre PAS dans la
/// liste : sans les mêmes feuilles, une comparaison cellule à cellule ne veut
/// rien dire. Une divergence de structure fait échouer le test sur-le-champ.
fn comparer(scenario: &str, nav: &[FeuilleRelevee], hub: &[FeuilleRelevee]) -> Vec<Ecart> {
    let noms_nav: Vec<&str> = nav.iter().map(|f| f.nom.as_str()).collect();
    let noms_hub: Vec<&str> = hub.iter().map(|f| f.nom.as_str()).collect();
    assert_eq!(
        noms_nav, noms_hub,
        "[{scenario}] les deux générateurs ne posent pas les mêmes onglets, \
         ni dans le même ordre — la comparaison des cellules n'a plus de sens"
    );

    let mut ecarts = Vec::new();
    for (a, b) in nav.iter().zip(hub.iter()) {
        let refs: BTreeSet<&String> = a.cellules.keys().chain(b.cellules.keys()).collect();
        // Tri par ligne puis colonne : l'ordre alphabétique mettrait A10 avant
        // A9, et la liste doit se lire comme la feuille.
        let mut triees: Vec<&String> = refs.into_iter().collect();
        triees.sort_by_key(|r| ordre_cellule(r));
        for r in triees {
            let x = a.cellules.get(r);
            let y = b.cellules.get(r);
            if x != y {
                ecarts.push(Ecart {
                    scenario: scenario.to_string(),
                    feuille: a.nom.clone(),
                    ou: r.clone(),
                    navigateur: x.map(|c| c.to_string()).unwrap_or_else(|| "(vide)".into()),
                    hub: y.map(|c| c.to_string()).unwrap_or_else(|| "(vide)".into()),
                });
            }
        }

        let n = a.largeurs.len().max(b.largeurs.len());
        for i in 0..n {
            let x = a.largeurs.get(i).copied();
            let y = b.largeurs.get(i).copied();
            if x != y {
                ecarts.push(Ecart {
                    scenario: scenario.to_string(),
                    feuille: a.nom.clone(),
                    ou: format!("(largeur colonne {})", i + 1),
                    navigateur: x
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "(aucune)".into()),
                    hub: y
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "(aucune)".into()),
                });
            }
        }

        if a.images != b.images {
            ecarts.push(Ecart {
                scenario: scenario.to_string(),
                feuille: a.nom.clone(),
                ou: "(dessins ancrés)".to_string(),
                navigateur: a.images.to_string(),
                hub: b.images.to_string(),
            });
        }
    }
    ecarts
}

/// (ligne, colonne) d'une référence « B12 », pour trier comme on lit.
fn ordre_cellule(r: &str) -> (u32, String) {
    let col: String = r.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let ligne: u32 = r[col.len()..].parse().unwrap_or(0);
    (ligne, col)
}

#[test]
fn les_deux_generateurs_rendent_le_meme_classeur_aux_ecarts_assumes_pres() {
    let nav = releve_navigateur();
    let mut observes: Vec<Ecart> = Vec::new();
    for scenario in SCENARIOS {
        let c = charge(scenario);
        let feuilles_nav = nav
            .scenarios
            .get(scenario)
            .unwrap_or_else(|| panic!("le relevé du navigateur ne porte pas « {scenario} »"));
        observes.extend(comparer(scenario, feuilles_nav, &construire(&c)));
    }

    if std::env::var("PARITE_CLASSEUR").as_deref() == Ok("maj") {
        regenerer(&observes);
    }
    let attendus = ecarts_attendus();

    // ⚠️ Égalité stricte, dans les deux sens. Un écart NOUVEAU est une
    // divergence qu'on n'a pas décidée ; un écart DISPARU veut dire que
    // quelqu'un a rapproché les deux générateurs — c'est une bonne nouvelle,
    // mais elle se constate et se documente, elle ne se subit pas.
    let connus: Vec<Ecart> = attendus.iter().map(|e| e.ecart.clone()).collect();
    let nouveaux: Vec<&Ecart> = observes.iter().filter(|e| !connus.contains(e)).collect();
    let disparus: Vec<&Ecart> = connus.iter().filter(|e| !observes.contains(e)).collect();
    assert!(
        nouveaux.is_empty() && disparus.is_empty(),
        "la parité a bougé.\n\nÉcarts NOUVEAUX ({}) :\n{}\nÉcarts DISPARUS ({}) :\n{}\n\
         Relire `tests/parite-classeur/ecarts-attendus.json`, puis régénérer avec \
         PARITE_CLASSEUR=maj si le changement est voulu.",
        nouveaux.len(),
        rendu(&nouveaux),
        disparus.len(),
        rendu(&disparus),
    );
}

fn rendu(ecarts: &[&Ecart]) -> String {
    ecarts
        .iter()
        .map(|e| {
            format!(
                "  [{}] {}!{} — navigateur : {} | hub : {}\n",
                e.scenario, e.feuille, e.ou, e.navigateur, e.hub
            )
        })
        .collect()
}

fn ecarts_attendus() -> Vec<EcartAttendu> {
    let chemin = racine().join("ecarts-attendus.json");
    serde_json::from_str(&std::fs::read_to_string(&chemin).unwrap_or_else(|e| {
        panic!(
            "liste des écarts assumés absente ({}) : {e}",
            chemin.display()
        )
    }))
    .expect("liste des écarts assumés mal formée")
}

/// Réécrit la liste **en conservant les qualifications déjà relues**.
///
/// ⚠️ Un écart inédit ressort en `À QUALIFIER`, jamais rangé d'office : c'est
/// exactement ce qu'on veut voir échouer. Qualifier à la place de l'humain
/// serait ranger une divergence sous le tapis avec l'aide de l'outil.
fn regenerer(observes: &[Ecart]) {
    let anciens = ecarts_attendus();
    let sortie: Vec<EcartAttendu> = observes
        .iter()
        .map(|e| EcartAttendu {
            nature: anciens
                .iter()
                .find(|a| a.ecart == *e)
                .map(|a| a.nature.clone())
                .unwrap_or_else(|| "À QUALIFIER".to_string()),
            ecart: e.clone(),
        })
        .collect();
    std::fs::write(
        racine().join("ecarts-attendus.json"),
        serde_json::to_string_pretty(&sortie).unwrap() + "\n",
    )
    .unwrap();
}

/// Chaque écart porte une qualification connue — aucune n'est laissée en
/// suspens.
#[test]
fn chaque_ecart_est_qualifie() {
    for e in ecarts_attendus() {
        assert!(
            NATURES.contains(&e.nature.as_str()),
            "écart non qualifié ({}) : [{}] {}!{} — navigateur : {} | hub : {}\n\
             Le ranger dans {NATURES:?} après l'avoir compris, pas avant.",
            e.nature,
            e.ecart.scenario,
            e.ecart.feuille,
            e.ecart.ou,
            e.ecart.navigateur,
            e.ecart.hub
        );
    }
}

/// 🔴 **Aucun écart ne doit être un défaut de chiffre.**
///
/// ⚠️ Ce test ÉCHOUE tant qu'un écart reste qualifié `defaut` — et c'est le
/// cas au moment où il est écrit. Il ne s'agit pas d'un test cassé : c'est le
/// refus de la bascule, rendu exécutable. Le jour où le hub et le navigateur
/// rendent le même chiffre, il passe au vert et on retire l'entrée de
/// `ecarts-attendus.json`.
///
/// ⚠️ Sans cette garde, `ecarts-attendus.json` deviendrait le tapis sous lequel
/// on glisse une divergence de chiffre : la liste peut s'allonger de dates et
/// de largeurs, jamais de nombres.
#[test]
fn aucun_ecart_nest_un_defaut_de_chiffre() {
    let defauts: Vec<EcartAttendu> = ecarts_attendus()
        .into_iter()
        .filter(|e| e.nature == "defaut")
        .collect();
    let liste: String = defauts
        .iter()
        .map(|e| {
            format!(
                "  [{}] {}!{} — navigateur : {} | hub : {}\n",
                e.ecart.scenario, e.ecart.feuille, e.ecart.ou, e.ecart.navigateur, e.ecart.hub
            )
        })
        .collect();
    assert!(
        defauts.is_empty(),
        "{} cellule(s) où le hub et le navigateur ne rendent pas le même \
         chiffre — la bascule du hub web reste REFUSÉE :\n{liste}\n\
         Cause connue : `report_i18n::round_to` fait `(v * 10^n).round() / 10^n`, \
         là où le navigateur fait `Number(v.toFixed(n))`. Les deux diffèrent dès \
         que la multiplication remonte une valeur juste sous la demi-unité \
         au-dessus (712.05 → 712,0 côté navigateur, 712,1 côté hub).",
        defauts.len(),
    );
}

// ── Les invariants qui ne se négocient pas ───────────────────────────────
//
// ⚠️ Une parité parfaite entre deux classeurs également fautifs ne vaudrait
// rien. Ces trois-là sont vérifiés des DEUX côtés, avec les mêmes mots — le
// test du navigateur porte les siens.

#[test]
fn le_hub_ne_pose_aucune_ligne_de_total_ni_de_moyenne() {
    // Deux cibles à 100 % et 0 % ne font pas « 50 % de disponibilité chez
    // Durand » : le SLA global d'un client est refusé, pas moyenné.
    const INTERDITS: [&str; 8] = [
        "total", "totaux", "moyenne", "somme", "cumul", "average", "sum", "global",
    ];
    for scenario in SCENARIOS {
        for f in construire(&charge(scenario)) {
            for (r, c) in &f.cellules {
                if let Cellule::Texte { s } = c {
                    let bas = s.trim().to_lowercase();
                    for mot in INTERDITS {
                        assert!(
                            !bas.starts_with(mot),
                            "[{scenario}] {}!{r} commence par « {mot} » : {s:?}",
                            f.nom
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn le_hub_ecrit_lindetermine_et_la_couverture_en_chaines_jamais_en_nombres() {
    // 🔴 Un tableur agrège ce qui est numérique : une somme de colonne ferait
    // renaître le chiffre que le hub a refusé de calculer.
    let feuilles = construire(&charge("solo"));
    let synthese = &feuilles[0];
    assert_eq!(
        synthese.cellules.get("C10"),
        Some(&Cellule::Texte {
            s: "indéterminé".into()
        }),
        "la disponibilité d'une cible sans le moindre verdict doit être un MOT"
    );
    assert_eq!(
        synthese.cellules.get("J8"),
        Some(&Cellule::Texte {
            s: "92.6 % de la période mesurée".into()
        })
    );
    // Période entièrement mesurée : rien à dire, et surtout pas un zéro.
    assert_eq!(synthese.cellules.get("J9"), None);
    assert_eq!(
        synthese.cellules.get("J10"),
        Some(&Cellule::Texte {
            s: "période indéterminée".into()
        })
    );

    let adresses = feuilles.last().unwrap();
    assert_eq!(adresses.nom, "SLA par IP publique");
    assert_eq!(
        adresses.cellules.get("C4"),
        Some(&Cellule::Texte {
            s: "Indéterminé".into()
        }),
        "la tranche qu'aucun intervalle ne couvre se lit, elle ne se devine pas"
    );
    assert_eq!(
        adresses.cellules.get("H4"),
        Some(&Cellule::Texte {
            s: "100.00 %".into()
        }),
        "le pourcentage par adresse est une chaîne : la colonne ne se totalise pas"
    );
}

#[test]
fn le_hub_date_ses_coupures_et_nomme_son_fuseau() {
    // ⚠️ C'est l'écart assumé, pris par le bon bout : une heure sans fuseau,
    // dans un document qui liste des coupures datées, se conteste au premier
    // décalage horaire.
    let feuilles = construire(&charge("solo"));
    let internet = feuilles
        .iter()
        .find(|f| f.nom == "Accès internet")
        .expect("la feuille de l'accès internet");
    // Ligne 13 : première coupure. Colonnes Début / Fin / Durée / Perdus.
    let debut = internet.cellules.get("A13").unwrap();
    let fin = internet.cellules.get("B13").unwrap();
    for c in [debut, fin] {
        match c {
            Cellule::Texte { s } => assert!(
                s.ends_with(" UTC"),
                "une coupure datée sans fuseau se conteste : {s:?}"
            ),
            autre => panic!("une date doit être du texte, pas {autre}"),
        }
    }
    assert_eq!(
        internet.cellules.get("C13"),
        Some(&Cellule::Texte { s: "20 min".into() })
    );
    // La coupure en cours n'est jamais close sur l'instant de l'export : ni sa
    // fin ni sa durée ne s'inventent.
    assert_eq!(
        internet.cellules.get("B14"),
        Some(&Cellule::Texte {
            s: "En cours".into()
        })
    );
    assert_eq!(
        internet.cellules.get("C14"),
        Some(&Cellule::Texte {
            s: "En cours".into()
        })
    );
}
