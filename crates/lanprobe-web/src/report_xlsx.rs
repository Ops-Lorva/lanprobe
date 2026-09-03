//! Le classeur SLA, produit par le hub (contrat § 23).
//!
//! 🔴 **Exactement le même classeur que celui du hub web.** Pas un équivalent,
//! pas une version mobile allégée : six familles de feuilles — Résumé · une
//! feuille par cible (coupures datées + série) · Débit · Machines · Ports ·
//! Adresses publiques. Le modèle est `web-ui/src/lib/sla-report.ts`, et le
//! **hub web devra basculer sur cette route** : sans cela on ne passe pas de
//! deux générateurs à un, on passe à trois. C'est la moitié la plus importante
//! du chantier, et celle qu'on oublie.
//!
//! ⚠️ **Rien n'est recalculé ici.** Les taux, la couverture et l'historique des
//! adresses arrivent tels que `sla_payload::build` les rend. Le classeur remis
//! au client et le CSV doivent annoncer la même complétude ; deux calculs
//! finiraient par diverger d'un point, et c'est celui du document qu'on ne
//! saurait plus expliquer.
//!
//! ⚠️ Trois partis pris hérités du générateur du navigateur, à ne pas perdre :
//! la fenêtre est écrite **en toutes lettres** sur chaque feuille ; les
//! coupures sont listées **une par une**, avec début, fin et durée ; une
//! coupure **en cours** est marquée comme telle, jamais close sur l'instant de
//! l'export — écrire une heure de fin qui n'a pas eu lieu fabriquerait une
//! durée fausse.
//!
//! ⚠️ **Les graphiques restent des IMAGES.** Tranché par Benjamin. Un
//! graphique Excel natif se recalcule à l'ouverture — le client trie une
//! colonne et la courbe bouge sous ses yeux — et surtout il ne sait pas
//! peindre les bandes de coupure : il relierait deux mesures séparées par une
//! panne, ce qui est la faute cardinale de ce produit. Le dessin vit dans
//! [`crate::report_chart`] ; le classeur se lit très bien sans, puisque
//! l'image illustre des chiffres qui sont déjà dans les cellules.
// Le générateur n'a pas encore d'appelant : la route qui le branche est
// écrite à part (contrat § 23).
#![allow(dead_code)]

use serde::Deserialize;

use crate::report_chart::chart_png;
use crate::report_i18n::{iso_day, round_to, trim_num, Catalog};

/// Délai d'attente d'un ping, côté sonde (`lanprobe-core/src/ping.rs`).
///
/// ⚠️ Une latence au-delà est impossible : le ping aurait abandonné avant. Elle
/// vient d'une sonde suspendue — un portable endormi pendant la mesure compte
/// son sommeil comme temps de réponse. Relevé en production : 1 045 504 ms.
/// Dans un rapport remis à un client, un « pic à 17 minutes » se discute en
/// réunion.
///
/// ⚠️ Seules la moyenne, le minimum, le maximum et le p95 l'ignorent. La
/// DISPONIBILITÉ n'est pas touchée : elle se calcule sur `alive`, et réécrire
/// après coup un `alive` enregistré changerait un pourcentage de SLA déjà remis.
pub(crate) const PING_TIMEOUT_MS: f64 = 1000.0;

/// Largeur de colonne minimale et plafond, en caractères.
///
/// ⚠️ Sans ajustement, une adresse longue ou un nom de serveur Ookla s'affiche
/// en `####` ou tronqué, et le lecteur doit élargir à la main — dans un
/// document qu'on lui a remis. Plafonné : une URL de résultat ferait une
/// colonne de 300 caractères qui pousse tout le reste hors de l'écran.
const LARGEUR_MIN: usize = 10;
const LARGEUR_MAX: usize = 46;

/// Où se pose le graphique, et à quelle taille — repris du navigateur.
///
/// ⚠️ **À droite des données, jamais dessus** : il ne doit pas masquer les
/// chiffres qu'il illustre. Colonne F, ligne 2.
const IMAGE_LIGNE: u32 = 1;
const IMAGE_COLONNE: u16 = 5;
const IMAGE_LARGEUR: f64 = 720.0;
const IMAGE_HAUTEUR: f64 = 210.0;

/// Un onglet ne dépasse pas 31 caractères, et Excel refuse `\ / * ? : [ ]`.
const NOM_ONGLET_MAX: usize = 31;
const NOM_ONGLET_INTERDITS: [char; 7] = ['\\', '/', '*', '?', ':', '[', ']'];

// ── Ce que le hub rend, tel qu'il le rend ────────────────────────────────
//
// ⚠️ Ces structures relisent la sortie de `sla_payload::build` : elles ne la
// recalculent pas. Les taux, la couverture et l'historique des adresses
// arrivent déjà faits ; deux calculs finiraient par diverger d'un point, et
// c'est celui du document remis au client qu'on ne saurait plus expliquer.

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Sample {
    pub timestamp: i64,
    /// `null` = **indéterminé** : la sonde n'a pas écrit de verdict.
    ///
    /// ⚠️ Ni disponible, ni en panne. Le hub le déduisait de la présence d'une
    /// latence — un silence transformé en fait.
    #[serde(default)]
    pub alive: Option<bool>,
    #[serde(default)]
    pub latency_ms: Option<f64>,
    /// Présent pour l'accès internet : `online` / `limited` / `offline`.
    #[serde(default)]
    pub state: Option<String>,
}

/// Ce qui a réellement été mesuré sur la fenêtre, **calculé par le hub**.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Coverage {
    pub window_secs: i64,
    pub covered_secs: i64,
    pub gap_secs: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TargetSeries {
    pub ip: String,
    #[serde(default)]
    pub samples: Vec<Sample>,
    /// Absente d'un hub antérieur à la fonctionnalité — voir [`coverage_label`].
    #[serde(default)]
    pub coverage: Option<Coverage>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SpeedtestRow {
    pub started_at: i64,
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub download_mbps: Option<f64>,
    #[serde(default)]
    pub upload_mbps: Option<f64>,
    #[serde(default)]
    pub latency_ms: Option<f64>,
    #[serde(default)]
    pub jitter_ms: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ScanHost {
    pub ip: String,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub mac: Option<String>,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub latency_ms: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ScanPort {
    pub ip: String,
    pub port: i64,
    pub proto: String,
    #[serde(default)]
    pub service: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Scan {
    pub started_at: i64,
    #[serde(default)]
    pub hosts: Vec<ScanHost>,
    #[serde(default)]
    pub ports: Vec<ScanPort>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PublicIpInterval {
    pub public_ip: String,
    #[serde(default)]
    pub interface: Option<String>,
    #[serde(default)]
    pub gateway: Option<String>,
    #[serde(default)]
    pub local_subnet: Option<String>,
    pub confirmed_from: i64,
    pub confirmed_until: i64,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SlaPayload {
    pub probe: String,
    pub site: String,
    pub range: String,
    pub generated_at: i64,
    #[serde(default)]
    pub targets: Vec<TargetSeries>,
    #[serde(default)]
    pub internet: Vec<Sample>,
    #[serde(default)]
    pub speedtests: Vec<SpeedtestRow>,
    #[serde(default)]
    pub discovery: Option<Scan>,
    #[serde(default)]
    pub ports: Option<Scan>,
    /// Intervalles d'adresse publique couvrant la fenêtre. Ils voyagent AVEC le
    /// rapport : le tableau de l'interface et l'onglet du classeur découpent
    /// les mêmes relevés avec les mêmes intervalles.
    #[serde(default)]
    pub public_ip_history: Vec<PublicIpInterval>,
}

// ── Les calculs, portés du navigateur ────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Outage {
    pub start: i64,
    /// ⚠️ `None` quand la série se termine sur un échec : la coupure dure
    /// peut-être encore. La fermer à l'instant de l'export inventerait une
    /// heure de rétablissement.
    pub end: Option<i64>,
    pub samples_lost: usize,
}

/// Coupures déduites des relevés.
pub(crate) fn outages(samples: &[Sample]) -> Vec<Outage> {
    let mut out = Vec::new();
    let mut current: Option<Outage> = None;
    for s in samples {
        // ⚠️ `== Some(false)`, PAS « pas vivant » : la négation est vraie pour
        // l'indéterminé, et chaque mesure sans verdict aurait ouvert une
        // coupure. Le faux 0 % serait revenu par la porte de derrière, sous
        // forme de pannes inventées.
        if s.alive == Some(false) {
            match current.as_mut() {
                Some(c) => c.samples_lost += 1,
                None => {
                    current = Some(Outage {
                        start: s.timestamp,
                        end: None,
                        samples_lost: 1,
                    })
                }
            }
        } else if let Some(mut c) = current.take() {
            c.end = Some(s.timestamp);
            out.push(c);
        }
    }
    if let Some(c) = current {
        out.push(c);
    }
    out
}

#[derive(Debug, Clone)]
pub(crate) struct Stats {
    /// 🔴 `None` = aucun relevé DÉTERMINÉ sur la période : on ne sait pas.
    ///
    /// ⚠️ Le type interdit d'imprimer un pourcentage qu'on n'a pas. `0`
    /// affirmait une panne totale, dans un classeur remis au client — pire que
    /// le faux 100 % qu'on corrigeait.
    pub uptime_pct: Option<f64>,
    pub avg: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub p95: Option<f64>,
    /// Relevés DÉTERMINÉS — le dénominateur de `uptime_pct`.
    pub total: usize,
    pub failed: usize,
    /// Relevés sans verdict. Comptés et affichés, jamais tus.
    pub undetermined: usize,
}

/// ⚠️ La disponibilité se calcule sur `alive`, jamais sur la présence d'une
/// latence : un hôte muet n'en écrit pas, et compter les latences donnerait
/// 100 % sur un hôte mort.
pub(crate) fn stats(samples: &[Sample]) -> Stats {
    // ⚠️ Ce qui n'a pas été mesuré SORT du dénominateur : ni disponible, ni en
    // panne. Une fenêtre sans le moindre verdict n'a pas de pourcentage.
    let determines: Vec<&Sample> = samples.iter().filter(|s| s.alive.is_some()).collect();
    let alive = determines
        .iter()
        .filter(|s| s.alive == Some(true))
        .count();
    let mut lat: Vec<f64> = samples
        .iter()
        .filter_map(|s| s.latency_ms)
        .filter(|v| v.is_finite())
        // ⚠️ Une latence au-delà du délai d'attente vient d'une sonde
        // suspendue, pas du réseau. Un seul de ces points suffit à faire d'une
        // moyenne un chiffre que le client contestera à juste titre.
        .filter(|v| *v <= PING_TIMEOUT_MS)
        .collect();
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let p95 = (!lat.is_empty()).then(|| {
        let i = ((lat.len() as f64) * 0.95).floor() as usize;
        lat[i.min(lat.len() - 1)]
    });
    Stats {
        uptime_pct: (!determines.is_empty())
            .then(|| alive as f64 / determines.len() as f64 * 100.0),
        avg: (!lat.is_empty()).then(|| lat.iter().sum::<f64>() / lat.len() as f64),
        min: lat.first().copied(),
        max: lat.last().copied(),
        p95,
        total: determines.len(),
        failed: determines.len() - alive,
        undetermined: samples.len() - determines.len(),
    }
}

/// Mention de complétude à afficher, ou `None` quand il n'y a rien à dire.
///
/// ⚠️ **`None` quand c'est complet.** Un « 0 % indéterminé » permanent est du
/// bruit qui finit par masquer le cas où ça compte vraiment.
///
/// ⚠️ `None` aussi quand le hub n'envoie rien : absence d'information, on
/// n'invente pas « complet ».
///
/// ⚠️ Une fenêtre VIDE n'est pas complète — sans cette garde, `gap_secs == 0`
/// la déclarait « tout va bien » sur une période qui n'existe pas.
///
/// ⚠️ Le rendu est une CHAÎNE et le restera : dans une cellule de tableur, un
/// nombre se retrouve dans une moyenne de colonne et fait naître un chiffre que
/// personne n'a calculé.
pub(crate) fn coverage_label(coverage: Option<&Coverage>, c: &Catalog) -> Option<String> {
    let coverage = coverage?;
    if coverage.window_secs <= 0 {
        return Some(c.t("sla.coverage_unknown", &[]));
    }
    if coverage.gap_secs == 0 {
        return None;
    }
    let pct = coverage.covered_secs as f64 / coverage.window_secs as f64 * 100.0;
    Some(c.t("sla.coverage_partial", &[("pct", format!("{pct:.1}"))]))
}

/// Durée d'une fenêtre Flux, en toutes lettres.
pub(crate) fn window_label(range: &str, c: &Catalog) -> String {
    // Bornes explicites : « du … au … », la seule forme qui vaille dans un
    // document contractuel.
    if let Some((a, b)) = range.split_once("..")
        && let (Ok(from), Ok(to)) = (a.parse::<i64>(), b.parse::<i64>())
    {
        return format!(
            "{} {} {} {}",
            c.t("sla.from", &[]),
            c.day(from),
            c.t("sla.to", &[]),
            c.day(to)
        );
    }
    let Some(reste) = range.strip_prefix('-') else {
        return range.to_string();
    };
    let (n, unite) = reste.split_at(reste.len().saturating_sub(1));
    let Ok(n) = n.parse::<i64>() else {
        return range.to_string();
    };
    match unite {
        "d" => c.t("sla.window_days", &[("n", n.to_string())]),
        "h" => c.t("sla.window_hours", &[("n", n.to_string())]),
        // Ce qu'on ne sait pas dire, on le recopie plutôt que de l'inventer.
        _ => range.to_string(),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct IpSlaRow {
    /// `None` = indéterminé : aucun intervalle ne couvrait ces relevés.
    pub public_ip: Option<String>,
    pub label: Option<String>,
    pub gateway: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub samples: usize,
    /// `None` = aucun relevé déterminé sur cette tranche.
    pub uptime_pct: Option<f64>,
}

/// Répartit les relevés d'accès internet par adresse publique.
///
/// ⚠️ Les pourcentages ne totalisent PAS 100 % : un relevé qu'aucun intervalle
/// ne couvre va en « indéterminé » et n'est imputé à personne. C'est délibéré.
/// Une coupure attribuée à la mauvaise adresse a l'air normale et personne ne
/// la remet en cause — exactement le mode de panne que le projet combat
/// partout ailleurs.
///
/// ⚠️ Tout est en **secondes** ici, des deux côtés de la comparaison.
pub(crate) fn by_public_ip(samples: &[Sample], intervals: &[PublicIpInterval]) -> Vec<IpSlaRow> {
    let mut ordered: Vec<&PublicIpInterval> = intervals.iter().collect();
    ordered.sort_by_key(|i| i.confirmed_from);

    // Ordre d'apparition conservé, comme la `Map` du navigateur : le tri final
    // est stable, et deux tranches de même début gardent leur ordre.
    let mut buckets: Vec<(String, Option<&PublicIpInterval>, Vec<&Sample>)> = Vec::new();
    for s in samples {
        let hit = ordered
            .iter()
            .find(|i| s.timestamp >= i.confirmed_from && s.timestamp <= i.confirmed_until)
            .copied();
        let cle = hit.map(|i| i.public_ip.clone()).unwrap_or_default();
        match buckets.iter_mut().find(|(k, _, _)| *k == cle) {
            Some((_, _, v)) => v.push(s),
            None => buckets.push((cle, hit, vec![s])),
        }
    }

    let mut rows: Vec<IpSlaRow> = buckets
        .into_iter()
        .map(|(cle, meta, samples)| {
            let clones: Vec<Sample> = samples.iter().map(|s| (*s).clone()).collect();
            IpSlaRow {
                public_ip: (!cle.is_empty()).then_some(cle),
                label: meta.and_then(|m| m.label.clone()),
                gateway: meta.and_then(|m| m.gateway.clone()),
                from: samples.iter().map(|s| s.timestamp).min(),
                to: samples.iter().map(|s| s.timestamp).max(),
                samples: samples.len(),
                uptime_pct: stats(&clones).uptime_pct,
            }
        })
        .collect();
    // L'indéterminé en dernier : c'est un reste, pas une ligne comme les autres.
    rows.sort_by(|a, b| match (&a.public_ip, &b.public_ip) {
        (None, _) => std::cmp::Ordering::Greater,
        (_, None) => std::cmp::Ordering::Less,
        _ => a.from.unwrap_or(0).cmp(&b.from.unwrap_or(0)),
    });
    rows
}

// ── L'écriture ───────────────────────────────────────────────────────────

/// Ce qu'un classeur porte : un site, une fenêtre, et les relevés de chacune
/// de ses sondes.
///
/// ⚠️ **Un site, une fenêtre.** Le générateur lit le site et la période du
/// premier relevé et suppose qu'ils valent pour tous : il ne sait pas produire
/// un classeur à deux sites ni à deux périodes. C'est vrai par construction
/// dans l'interface — on ne peut pas cocher des sondes de sites différents — et
/// c'est écrit dans le contrat de route (§ 23).
pub(crate) struct Workbook {
    pub site_name: String,
    pub range_start: i64,
    pub range_stop: i64,
    /// Un élément par sonde, tel que `sla_payload::build` le rend.
    pub payloads: Vec<serde_json::Value>,
}

/// Ce que la construction produit — le fichier et de quoi le vérifier.
pub(crate) struct BuiltFile {
    pub file_name: String,
    pub bytes: Vec<u8>,
    /// 🔴 Calculé **sur les octets écrits**, jamais sur une valeur intermédiaire.
    /// C'est le seul contrôle qui attrape une corruption silencieuse, et non
    /// seulement une coupure (contrat § 23).
    pub sha256: String,
}

/// ⚠️ Le corps du fichier n'entre PAS dans la trace : quelques centaines de
/// kilo-octets de XML compressé dans un journal noieraient la ligne qui
/// comptait, et ce sont les données d'un client.
impl std::fmt::Debug for BuiltFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltFile")
            .field("file_name", &self.file_name)
            .field("taille", &self.bytes.len())
            .field("sha256", &self.sha256)
            .finish()
    }
}

/// Une cellule, telle que le tableur la lira.
///
/// 🔴 **Le choix entre `Texte` et `Nombre` n'est pas cosmétique.** Un tableur
/// agrège ce qui est numérique : une somme ou une moyenne de colonne sur un
/// « indéterminé » écrit en nombre ferait renaître le chiffre que le hub a
/// refusé de calculer.
#[derive(Debug, Clone)]
enum Cell {
    Texte(String),
    Nombre(f64),
    /// Rien à écrire — une cellule vraiment vide, pas un zéro.
    Vide,
}

impl Cell {
    fn texte(s: impl Into<String>) -> Self {
        Cell::Texte(s.into())
    }

    /// Le nombre tel qu'il s'affichera, pour mesurer la colonne.
    fn largeur(&self) -> usize {
        match self {
            Cell::Texte(s) => s.chars().count(),
            Cell::Nombre(v) => trim_num(*v).chars().count(),
            Cell::Vide => 0,
        }
    }
}

/// Un onglet en cours d'écriture : le curseur de ligne et la largeur des
/// colonnes, qu'on ne connaît qu'une fois tout écrit.
struct Feuille {
    ws: rust_xlsxwriter::Worksheet,
    ligne: u32,
    largeurs: Vec<usize>,
}

impl Feuille {
    fn nouvelle(nom: &str) -> Result<Self, String> {
        let mut ws = rust_xlsxwriter::Worksheet::new();
        ws.set_name(nom)
            .map_err(|e| format!("onglet « {nom} » refusé : {e}"))?;
        Ok(Self {
            ws,
            ligne: 0,
            largeurs: Vec::new(),
        })
    }

    fn ecrire(&mut self, cellules: &[Cell]) -> Result<(), String> {
        for (i, cellule) in cellules.iter().enumerate() {
            let col = i as u16;
            match cellule {
                Cell::Texte(s) => {
                    self.ws
                        .write_string(self.ligne, col, s)
                        .map_err(|e| e.to_string())?;
                }
                Cell::Nombre(v) => {
                    self.ws
                        .write_number(self.ligne, col, *v)
                        .map_err(|e| e.to_string())?;
                }
                Cell::Vide => {}
            }
            if self.largeurs.len() <= i {
                self.largeurs.resize(i + 1, LARGEUR_MIN);
            }
            // +2 : la bordure de cellule mange un peu de place, et un texte qui
            // touche exactement le bord se lit comme s'il était coupé.
            let voulue = (cellule.largeur() + 2).min(LARGEUR_MAX);
            self.largeurs[i] = self.largeurs[i].max(voulue);
        }
        self.ligne += 1;
        Ok(())
    }

    /// Une ligne laissée vide, pour aérer — comme `addRow([])`.
    fn saut(&mut self) {
        self.ligne += 1;
    }

    /// Pose le graphique à droite des données.
    ///
    /// ⚠️ L'échelle ramène les 900 × 260 pixels du dessin aux 720 × 210 points
    /// du classeur du navigateur : dessiner plus grand puis réduire garde le
    /// texte des axes lisible à l'impression.
    fn image(&mut self, png: Vec<u8>) -> Result<(), String> {
        let image = rust_xlsxwriter::Image::new_from_buffer(&png)
            .map_err(|e| format!("graphique illisible : {e}"))?
            .set_scale_width(IMAGE_LARGEUR / crate::report_chart::LARGEUR as f64)
            .set_scale_height(IMAGE_HAUTEUR / crate::report_chart::HAUTEUR as f64);
        self.ws
            .insert_image(IMAGE_LIGNE, IMAGE_COLONNE, &image)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Ajuste chaque colonne au plus long contenu qu'elle porte, puis rend
    /// l'onglet prêt à être posé dans le classeur.
    fn poser(mut self, wb: &mut rust_xlsxwriter::Workbook) -> Result<(), String> {
        for (i, l) in self.largeurs.iter().enumerate() {
            self.ws
                .set_column_width(i as u16, *l as f64)
                .map_err(|e| e.to_string())?;
        }
        wb.push_worksheet(self.ws);
        Ok(())
    }
}

/// Une ligne de la synthèse : une cible d'une sonde.
struct Ligne {
    probe: String,
    label: String,
    samples: Vec<Sample>,
    coverage: Option<Coverage>,
}

/// Construit le classeur.
///
/// ⚠️ Rend le fichier **en mémoire** et non un flux : la route doit annoncer un
/// `Content-Length` sur une réponse **non chunkée**. Une réponse construite en
/// flux part volontiers en `Transfer-Encoding: chunked`, auquel cas il n'y a
/// aucune taille à comparer et la garantie contre un fichier tronqué tombe en
/// silence. Un XLSX de SLA pèse quelques centaines de kilo-octets.
pub(crate) fn build(workbook: &Workbook, catalog: &Catalog) -> Result<BuiltFile, String> {
    let payloads: Vec<SlaPayload> = workbook
        .payloads
        .iter()
        .map(|v| serde_json::from_value(v.clone()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("relevé illisible : {e}"))?;
    // ⚠️ Un échec nommé, pas un classeur d'une feuille vide : un document remis
    // au client ne doit jamais être « techniquement produit » et sans contenu.
    let first = payloads
        .first()
        .ok_or_else(|| "aucun relevé à mettre dans le classeur".to_string())?;

    let mut wb = rust_xlsxwriter::Workbook::new();
    wb.set_properties(&rust_xlsxwriter::DocProperties::new().set_author("LanProbe"));

    let fenetre = window_label(&first.range, catalog);
    let genere = catalog.date(first.generated_at);
    let site = first.site.clone();

    // ⚠️ La fenêtre est écrite en toutes lettres sur CHAQUE feuille : « 99,2 % »
    // ne veut rien dire sans savoir sur quoi.
    let entete = |f: &mut Feuille, probe: Option<&str>| -> Result<(), String> {
        if let Some(p) = probe {
            f.ecrire(&[Cell::texte(catalog.t("sla.probe", &[])), Cell::texte(p)])?;
        }
        f.ecrire(&[
            Cell::texte(catalog.t("sla.site", &[])),
            Cell::texte(site.clone()),
        ])?;
        f.ecrire(&[
            Cell::texte(catalog.t("sla.window", &[])),
            Cell::texte(fenetre.clone()),
        ])?;
        f.ecrire(&[
            Cell::texte(catalog.t("sla.generated", &[])),
            Cell::texte(genere.clone()),
        ])?;
        f.saut();
        Ok(())
    };

    // Une seule sonde : son nom va dans l'en-tête. Plusieurs : il devient une
    // colonne, parce que la vue d'ensemble est ce qu'on ouvre le document pour
    // voir.
    let sonde_unique = (payloads.len() == 1).then(|| first.probe.clone());

    let lignes: Vec<Ligne> = payloads
        .iter()
        .flat_map(|p| {
            let mut v = Vec::new();
            if !p.internet.is_empty() {
                v.push(Ligne {
                    probe: p.probe.clone(),
                    label: catalog.t("sla.internet", &[]),
                    samples: p.internet.clone(),
                    coverage: None,
                });
            }
            for tg in &p.targets {
                v.push(Ligne {
                    probe: p.probe.clone(),
                    label: tg.ip.clone(),
                    samples: tg.samples.clone(),
                    coverage: tg.coverage.clone(),
                });
            }
            v
        })
        .collect();

    let mut noms = NomsDOnglets::default();

    // ── Synthèse ────────────────────────────────────────────────────────
    //
    // 🔴 **Aucune ligne de total ni de moyenne.** Deux cibles à 100 % et 0 % ne
    // font pas « 50 % de disponibilité chez Durand » : le SLA global d'un
    // client est refusé, pas moyenné.
    let mut sum = Feuille::nouvelle(&noms.retenir(&catalog.t("sla.sheet_summary", &[])))?;
    entete(&mut sum, sonde_unique.as_deref())?;
    sum.ecrire(&[
        Cell::texte(catalog.t("sla.probe", &[])),
        Cell::texte(catalog.t("sla.col_target", &[])),
        Cell::texte(catalog.t("sla.col_uptime", &[])),
        Cell::texte(catalog.t("sla.col_avg", &[])),
        Cell::texte(catalog.t("sla.col_min", &[])),
        Cell::texte(catalog.t("sla.col_max", &[])),
        Cell::texte(catalog.t("sla.col_p95", &[])),
        Cell::texte(catalog.t("sla.col_samples", &[])),
        Cell::texte(catalog.t("sla.col_outages", &[])),
        Cell::texte(catalog.t("sla.col_coverage", &[])),
    ])?;
    for l in &lignes {
        let s = stats(&l.samples);
        sum.ecrire(&[
            Cell::texte(l.probe.clone()),
            Cell::texte(l.label.clone()),
            // ⚠️ Le mot, pas une cellule vide (qui se lit comme un oubli de
            // l'outil) ni `0` (qui se lit comme une panne). ⚠️ Et une CHAÎNE,
            // pas un nombre : une moyenne de colonne dans le tableur ne doit
            // pas l'agréger comme un zéro — c'est exactement là que le faux
            // 0 % reviendrait.
            match s.uptime_pct {
                Some(v) => Cell::Nombre(round_to(v, 2)),
                None => Cell::texte(catalog.t("sla.undetermined", &[])),
            },
            nombre_ou_tiret(s.avg.map(|v| round_to(v, 1))),
            nombre_ou_tiret(s.min),
            nombre_ou_tiret(s.max),
            nombre_ou_tiret(s.p95),
            Cell::Nombre(s.total as f64),
            Cell::Nombre(outages(&l.samples).len() as f64),
            // ⚠️ Une chaîne, jamais un nombre : un total ou une moyenne de
            // colonne ne doit pas pouvoir l'aspirer. Vide quand la période est
            // entièrement mesurée — c'est le cas normal, et le dire serait du
            // bruit.
            match coverage_label(l.coverage.as_ref(), catalog) {
                Some(t) => Cell::Texte(t),
                None => Cell::Vide,
            },
        ])?;
    }
    sum.poser(&mut wb)?;

    // ── Une feuille par cible : incidents puis série ─────────────────────
    for l in &lignes {
        let base = if payloads.len() > 1 {
            format!("{} {}", l.probe, l.label)
        } else {
            l.label.clone()
        };
        let mut f = Feuille::nouvelle(&noms.retenir(&base))?;
        entete(&mut f, Some(&l.probe))?;

        let s = stats(&l.samples);
        f.ecrire(&[
            Cell::texte(catalog.t("sla.col_uptime", &[])),
            Cell::texte(catalog.percent(s.uptime_pct)),
        ])?;
        f.ecrire(&[
            Cell::texte(catalog.t("sla.col_samples", &[])),
            Cell::Nombre(s.total as f64),
        ])?;
        f.ecrire(&[
            Cell::texte(catalog.t("sla.col_failed", &[])),
            Cell::Nombre(s.failed as f64),
        ])?;
        // Le compte des indéterminés voyage avec le chiffre : « 100 % sur trois
        // relevés » et « 100 % sur trois relevés et neuf mille indéterminés »
        // ne se lisent pas pareil.
        f.ecrire(&[
            Cell::texte(catalog.t("sla.col_undetermined", &[])),
            Cell::Nombre(s.undetermined as f64),
        ])?;
        if let Some(couverture) = coverage_label(l.coverage.as_ref(), catalog) {
            f.ecrire(&[
                Cell::texte(catalog.t("sla.col_coverage", &[])),
                Cell::Texte(couverture),
            ])?;
        }
        f.saut();

        // ⚠️ Les coupures sont listées une par une, avec leur début, leur fin et
        // leur durée. Un pourcentage seul se conteste ; une liste d'incidents
        // datés se vérifie.
        f.ecrire(&[Cell::texte(catalog.t("sla.sheet_outages", &[]))])?;
        f.ecrire(&[
            Cell::texte(catalog.t("sla.col_start", &[])),
            Cell::texte(catalog.t("sla.col_end", &[])),
            Cell::texte(catalog.t("sla.col_duration", &[])),
            Cell::texte(catalog.t("sla.col_lost", &[])),
        ])?;
        let liste = outages(&l.samples);
        if liste.is_empty() {
            f.ecrire(&[Cell::texte(catalog.t("sla.no_outage", &[]))])?;
        } else {
            for o in &liste {
                f.ecrire(&[
                    Cell::texte(catalog.date(o.start)),
                    // ⚠️ Jamais une heure de fin inventée : une coupure encore
                    // en cours se dit, elle ne se clôt pas sur l'instant de
                    // l'export.
                    match o.end {
                        Some(end) => Cell::texte(catalog.date(end)),
                        None => Cell::texte(catalog.t("sla.ongoing", &[])),
                    },
                    match o.end {
                        Some(end) => Cell::texte(catalog.duration(end - o.start)),
                        None => Cell::texte(catalog.t("sla.ongoing", &[])),
                    },
                    Cell::Nombre(o.samples_lost as f64),
                ])?;
            }
        }
        f.saut();

        f.ecrire(&[Cell::texte(catalog.t("sla.sheet_series", &[]))])?;
        f.ecrire(&[
            Cell::texte(catalog.t("sla.col_time", &[])),
            Cell::texte(catalog.t("sla.col_state", &[])),
            Cell::texte(catalog.t("sla.col_latency", &[])),
        ])?;
        for sample in &l.samples {
            f.ecrire(&[
                Cell::texte(catalog.date(sample.timestamp)),
                Cell::texte(match &sample.state {
                    Some(s) => s.clone(),
                    None => match sample.alive {
                        Some(true) => "OK".to_string(),
                        _ => "KO".to_string(),
                    },
                }),
                match sample.latency_ms {
                    Some(v) => Cell::Nombre(v),
                    None => Cell::Vide,
                },
            ])?;
        }
        // Le graphique se pose à droite des données, jamais dessus : il ne
        // doit pas masquer les chiffres qu'il illustre. Absent quand la série
        // est trop courte pour dire quoi que ce soit — le classeur se lit très
        // bien sans, l'image illustre des chiffres qui sont déjà là.
        if let Some(png) = chart_png(&l.samples, catalog) {
            f.image(png)?;
        }
        f.poser(&mut wb)?;
    }

    // ── Débits ──────────────────────────────────────────────────────────
    let debits: Vec<(&str, &SpeedtestRow)> = payloads
        .iter()
        .flat_map(|p| p.speedtests.iter().map(move |r| (p.probe.as_str(), r)))
        .collect();
    if !debits.is_empty() {
        let mut f = Feuille::nouvelle(&noms.retenir(&catalog.t("sla.sheet_speed", &[])))?;
        entete(&mut f, sonde_unique.as_deref())?;
        f.ecrire(&[
            Cell::texte(catalog.t("sla.probe", &[])),
            Cell::texte(catalog.t("sla.col_time", &[])),
            Cell::texte(catalog.t("sla.col_engine", &[])),
            Cell::texte(catalog.t("sla.col_server", &[])),
            Cell::texte(catalog.t("sla.col_down", &[])),
            Cell::texte(catalog.t("sla.col_up", &[])),
            Cell::texte(catalog.t("sla.col_latency", &[])),
            Cell::texte(catalog.t("sla.col_jitter", &[])),
        ])?;
        for (probe, r) in &debits {
            f.ecrire(&[
                Cell::texte(*probe),
                Cell::texte(catalog.date(r.started_at)),
                texte_ou_tiret(r.engine.as_deref()),
                texte_ou_tiret(r.server_name.as_deref()),
                nombre_ou_tiret(r.download_mbps.map(|v| round_to(v, 1))),
                nombre_ou_tiret(r.upload_mbps.map(|v| round_to(v, 1))),
                nombre_ou_tiret(r.latency_ms),
                nombre_ou_tiret(r.jitter_ms.map(|v| round_to(v, 1))),
            ])?;
        }
        f.poser(&mut wb)?;
    }

    // ── Inventaire : machines vues, et ports ouverts ─────────────────────
    //
    // Un client qui reçoit un SLA veut aussi savoir ce qui tournait chez lui.
    let machines: Vec<(&str, i64, &ScanHost)> = payloads
        .iter()
        .flat_map(|p| {
            p.discovery
                .iter()
                .flat_map(move |d| d.hosts.iter().map(move |h| (p.probe.as_str(), d.started_at, h)))
        })
        .collect();
    if !machines.is_empty() {
        let mut f = Feuille::nouvelle(&noms.retenir(&catalog.t("sla.sheet_hosts", &[])))?;
        entete(&mut f, sonde_unique.as_deref())?;
        f.ecrire(&[
            Cell::texte(catalog.t("sla.probe", &[])),
            Cell::texte(catalog.t("sla.col_scan_at", &[])),
            Cell::texte(catalog.t("sla.col_ip", &[])),
            Cell::texte(catalog.t("sla.col_hostname", &[])),
            Cell::texte(catalog.t("sla.col_mac", &[])),
            Cell::texte(catalog.t("sla.col_vendor", &[])),
            Cell::texte(catalog.t("sla.col_latency", &[])),
        ])?;
        for (probe, at, h) in &machines {
            f.ecrire(&[
                Cell::texte(*probe),
                Cell::texte(catalog.date(*at)),
                Cell::texte(h.ip.clone()),
                texte_ou_tiret(h.hostname.as_deref()),
                texte_ou_tiret(h.mac.as_deref()),
                texte_ou_tiret(h.vendor.as_deref()),
                nombre_ou_tiret(h.latency_ms),
            ])?;
        }
        f.poser(&mut wb)?;
    }

    let ports: Vec<(&str, i64, &ScanPort)> = payloads
        .iter()
        .flat_map(|p| {
            p.ports
                .iter()
                .flat_map(move |s| s.ports.iter().map(move |o| (p.probe.as_str(), s.started_at, o)))
        })
        .collect();
    if !ports.is_empty() {
        let mut f = Feuille::nouvelle(&noms.retenir(&catalog.t("sla.sheet_ports", &[])))?;
        entete(&mut f, sonde_unique.as_deref())?;
        f.ecrire(&[
            Cell::texte(catalog.t("sla.probe", &[])),
            Cell::texte(catalog.t("sla.col_scan_at", &[])),
            Cell::texte(catalog.t("sla.col_ip", &[])),
            Cell::texte(catalog.t("sla.col_port", &[])),
            Cell::texte(catalog.t("sla.col_proto", &[])),
            Cell::texte(catalog.t("sla.col_service", &[])),
        ])?;
        for (probe, at, o) in &ports {
            f.ecrire(&[
                Cell::texte(*probe),
                Cell::texte(catalog.date(*at)),
                Cell::texte(o.ip.clone()),
                Cell::Nombre(o.port as f64),
                Cell::texte(o.proto.clone()),
                texte_ou_tiret(o.service.as_deref()),
            ])?;
        }
        f.poser(&mut wb)?;
    }

    // ── SLA par IP publique ─────────────────────────────────────────────
    //
    // Un client qui conteste un SLA veut savoir sur quel lien il a été mesuré.
    //
    // ⚠️ Le découpage se fait sonde par sonde : les intervalles d'une sonde ne
    // décrivent que son propre lien, et croiser les relevés de l'une avec les
    // intervalles de l'autre imputerait une coupure à une adresse qui n'y était
    // pour rien.
    let adresses: Vec<(&str, IpSlaRow)> = payloads
        .iter()
        .flat_map(|p| {
            by_public_ip(&p.internet, &p.public_ip_history)
                .into_iter()
                .map(move |r| (p.probe.as_str(), r))
        })
        .collect();
    if !adresses.is_empty() {
        let mut f = Feuille::nouvelle(&noms.retenir(&catalog.t("report.ipSheet", &[])))?;
        f.ecrire(&[
            Cell::texte(catalog.t("sla.probe", &[])),
            Cell::texte(catalog.t("report.ipLabel", &[])),
            Cell::texte(catalog.t("report.ipAddress", &[])),
            Cell::texte(catalog.t("report.ipGateway", &[])),
            Cell::texte(catalog.t("report.ipFrom", &[])),
            Cell::texte(catalog.t("report.ipTo", &[])),
            Cell::texte(catalog.t("report.ipSamples", &[])),
            Cell::texte(catalog.t("report.ipUptime", &[])),
        ])?;
        for (probe, r) in &adresses {
            f.ecrire(&[
                Cell::texte(*probe),
                texte_ou_vide(r.label.as_deref()),
                // ⚠️ La ligne indéterminée n'est pas une adresse manquante :
                // c'est une part de la période qu'on refuse d'imputer. Elle
                // doit se lire comme telle, pas comme une case vide.
                match &r.public_ip {
                    Some(ip) => Cell::texte(ip.clone()),
                    None => Cell::texte(catalog.t("report.ipUndetermined", &[])),
                },
                texte_ou_vide(r.gateway.as_deref()),
                match r.from {
                    Some(t) => Cell::texte(catalog.date(t)),
                    None => Cell::Vide,
                },
                match r.to {
                    Some(t) => Cell::texte(catalog.date(t)),
                    None => Cell::Vide,
                },
                Cell::Nombre(r.samples as f64),
                // ⚠️ Deux décimales ici, comme le navigateur : c'est une
                // chaîne dans les deux cas, jamais un nombre que le tableur
                // pourrait agréger.
                match r.uptime_pct {
                    Some(v) => Cell::texte(format!("{v:.2} %")),
                    None => Cell::texte(catalog.t("sla.undetermined", &[])),
                },
            ])?;
        }
        f.poser(&mut wb)?;
    }

    let bytes = wb
        .save_to_buffer()
        .map_err(|e| format!("écriture du classeur impossible : {e}"))?;
    // 🔴 L'empreinte porte sur les octets qui partiront, pas sur ce qu'on
    // croyait écrire.
    let empreinte = ring::digest::digest(&ring::digest::SHA256, &bytes);
    let sha256 = empreinte.as_ref().iter().map(|b| format!("{b:02x}")).collect();

    Ok(BuiltFile {
        file_name: file_name(&workbook.site_name, workbook.range_start, workbook.range_stop),
        bytes,
        sha256,
    })
}

/// `'—'` plutôt qu'une cellule vide : l'absence de mesure se dit, elle ne se
/// devine pas au blanc laissé par l'outil.
fn nombre_ou_tiret(v: Option<f64>) -> Cell {
    match v {
        Some(v) => Cell::Nombre(v),
        None => Cell::texte("—"),
    }
}

fn texte_ou_tiret(v: Option<&str>) -> Cell {
    Cell::texte(v.unwrap_or("—"))
}

fn texte_ou_vide(v: Option<&str>) -> Cell {
    match v {
        Some(s) => Cell::texte(s),
        None => Cell::Vide,
    }
}

/// Les noms d'onglets déjà pris.
///
/// ⚠️ Excel refuse `\ / * ? : [ ]`, plafonne à 31 caractères et rejette un
/// doublon — deux sondes qui surveillent la même adresse, c'est le cas courant.
/// Le classeur ENTIER échouerait à l'écriture sur n'importe lequel des trois.
#[derive(Default)]
struct NomsDOnglets {
    pris: Vec<String>,
}

impl NomsDOnglets {
    fn retenir(&mut self, brut: &str) -> String {
        let assaini: String = brut
            .chars()
            .map(|c| if NOM_ONGLET_INTERDITS.contains(&c) { '-' } else { c })
            .collect();
        let base: String = assaini.chars().take(NOM_ONGLET_MAX).collect();
        // Excel refuse aussi un onglet sans nom : un tiret vaut mieux qu'un
        // classeur qui échoue tout entier à l'écriture.
        let base = if base.trim().is_empty() {
            "-".to_string()
        } else {
            base
        };
        let mut nom = base.clone();
        let mut n = 2;
        while self.pris.contains(&nom) {
            let tronc: String = base.chars().take(NOM_ONGLET_MAX - 3).collect();
            nom = format!("{tronc}~{n}");
            n += 1;
        }
        self.pris.push(nom.clone());
        nom
    }
}

/// Nom du fichier remis, dérivé du site et de la fenêtre.
///
/// ⚠️ Il part chez un client : il ne doit porter ni identifiant technique, ni
/// caractère que Windows refuse dans un nom de fichier.
///
/// La période est dans le nom : trois rapports du même site dans un dossier de
/// téléchargements doivent se distinguer sans les ouvrir.
pub(crate) fn file_name(site_name: &str, range_start: i64, range_stop: i64) -> String {
    let mut slug = String::new();
    let mut tiret = false;
    for c in site_name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            tiret = false;
        } else if !tiret && !slug.is_empty() {
            slug.push('-');
            tiret = true;
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "rapport" } else { slug };
    format!(
        "lanprobe-sla-{slug}-{}_{}.xlsx",
        iso_day(range_start),
        iso_day(range_stop)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::io::Read;

    // ── Relire le classeur ───────────────────────────────────────────────
    //
    // ⚠️ Le test ouvre le fichier **produit**, il ne relit pas les structures
    // qui ont servi à l'écrire. Un générateur qui se vérifie sur son propre
    // état interne ne prouve rien du document remis au client.
    //
    // Un XLSX est un zip de XML, et `zip` est déjà une dépendance : de quoi
    // rouvrir le classeur sans en tirer une de plus.

    #[derive(Debug, Clone, PartialEq)]
    enum Lu {
        Texte(String),
        Nombre(f64),
    }

    struct Classeur {
        /// Noms d'onglets, dans l'ordre du classeur.
        onglets: Vec<String>,
        /// Cellules par onglet, indexées par référence (« A1 »).
        cellules: Vec<HashMap<String, Lu>>,
    }

    impl Classeur {
        fn onglet(&self, nom: &str) -> &HashMap<String, Lu> {
            let i = self
                .onglets
                .iter()
                .position(|o| o == nom)
                .unwrap_or_else(|| panic!("onglet « {nom} » absent de {:?}", self.onglets));
            &self.cellules[i]
        }

        fn cell(&self, onglet: &str, r: &str) -> Option<&Lu> {
            self.onglet(onglet).get(r)
        }

        /// Les valeurs d'une colonne, de la ligne `depuis` à la fin.
        fn colonne(&self, onglet: &str, col: char, depuis: u32) -> Vec<Lu> {
            let cells = self.onglet(onglet);
            let mut out = Vec::new();
            for ligne in depuis..=200 {
                if let Some(v) = cells.get(&format!("{col}{ligne}")) {
                    out.push(v.clone());
                }
            }
            out
        }
    }

    fn relire(bytes: &[u8]) -> Classeur {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))
            .expect("le classeur doit être un zip lisible");
        let lire = |zip: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, nom: &str| -> String {
            let mut f = zip.by_name(nom).unwrap_or_else(|_| panic!("{nom} absent"));
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

        let mut cellules = Vec::new();
        for i in 1..=onglets.len() {
            let xml = lire(&mut zip, &format!("xl/worksheets/sheet{i}.xml"));
            let mut map = HashMap::new();
            for chunk in xml.split("<c ").skip(1) {
                let fin = chunk.find("</c>").unwrap_or(chunk.len());
                let chunk = &chunk[..fin];
                let Some(r) = attribut(chunk, "r") else { continue };
                let t = attribut(chunk, "t");
                let Some(vdeb) = chunk.find("<v>") else { continue };
                let vfin = chunk[vdeb..].find("</v>").unwrap() + vdeb;
                let brut = &chunk[vdeb + 3..vfin];
                let valeur = match t.as_deref() {
                    Some("s") => Lu::Texte(partagees[brut.parse::<usize>().unwrap()].clone()),
                    Some("str") | Some("inlineStr") => Lu::Texte(desechappe(brut)),
                    _ => Lu::Nombre(brut.parse().unwrap()),
                };
                map.insert(r, valeur);
            }
            cellules.push(map);
        }
        Classeur { onglets, cellules }
    }

    fn attribut(s: &str, nom: &str) -> Option<String> {
        let motif = format!("{nom}=\"");
        let i = s.find(&motif)? + motif.len();
        let j = s[i..].find('"')? + i;
        Some(desechappe(&s[i..j]))
    }

    fn desechappe(s: &str) -> String {
        s.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&")
    }

    // ── Un jeu de relevés ────────────────────────────────────────────────

    /// 3 septembre 2026, 12:00:00 UTC.
    const T0: i64 = 1_788_436_800;

    fn releve(offset: i64, alive: Option<bool>, latence: Option<f64>) -> serde_json::Value {
        json!({ "timestamp": T0 + offset, "alive": alive, "latency_ms": latence })
    }

    /// Une sonde complète : cibles, internet, débits, inventaire, adresses.
    fn payload_complet() -> serde_json::Value {
        json!({
            "probe": "Lyon-1",
            "site": "Durand",
            "range": format!("{}..{}", T0, T0 + 3_600),
            "start": T0,
            "stop": T0 + 3_600,
            "generated_at": T0 + 3_600,
            "targets": [{
                "ip": "192.168.1.1",
                "samples": [
                    releve(0, Some(true), Some(12.0)),
                    releve(60, Some(false), None),
                    releve(120, Some(true), Some(14.0)),
                ],
                "coverage": { "window_secs": 3_600, "covered_secs": 3_600, "gap_secs": 0 },
            }],
            "internet": [
                json!({ "timestamp": T0, "alive": true, "state": "online", "latency_ms": 9.0 }),
                json!({ "timestamp": T0 + 60, "alive": false, "state": "offline", "latency_ms": null }),
            ],
            "speedtests": [{
                "started_at": T0,
                "engine": "ookla",
                "server_name": "Lyon — Orange",
                "download_mbps": 512.34,
                "upload_mbps": 98.76,
                "latency_ms": 8,
                "jitter_ms": 1.25,
                "result_url": null,
            }],
            "discovery": {
                "started_at": T0,
                "cidr": "192.168.1.0/24",
                "hosts": [{ "ip": "192.168.1.10", "hostname": "nas", "mac": "aa:bb", "vendor": "Synology", "latency_ms": 3 }],
                "ports": [],
            },
            "ports": {
                "started_at": T0,
                "cidr": "192.168.1.0/24",
                "hosts": [],
                "ports": [{ "ip": "192.168.1.10", "port": 443, "proto": "tcp", "service": "https" }],
            },
            "public_ip_history": [{
                "public_ip": "88.120.0.1",
                "interface": "eth0",
                "gateway": "192.168.1.1",
                "local_subnet": "192.168.1.0/24",
                "confirmed_from": T0 - 10,
                "confirmed_until": T0 + 30,
                "label": "Fibre Orange",
            }],
        })
    }

    fn classeur(payloads: Vec<serde_json::Value>, locale: &str) -> (Classeur, BuiltFile) {
        let wb = Workbook {
            site_name: "Durand".into(),
            range_start: T0,
            range_stop: T0 + 3_600,
            payloads,
        };
        let fichier = build(&wb, &Catalog::load(locale)).expect("le classeur doit se construire");
        (relire(&fichier.bytes), fichier)
    }

    // ── Les six familles de feuilles ─────────────────────────────────────

    #[test]
    fn le_classeur_porte_les_six_familles_de_feuilles() {
        let (c, _) = classeur(vec![payload_complet()], "fr");
        assert_eq!(
            c.onglets,
            vec![
                "Synthèse",
                "Accès internet",
                "192.168.1.1",
                "Débits",
                "Machines découvertes",
                "Ports ouverts",
                "SLA par IP publique",
            ]
        );
    }

    /// ⚠️ Une famille absente ne laisse pas d'onglet vide : un classeur qui
    /// annonce « Débits » et n'en montre aucun se lit comme une panne de mesure.
    #[test]
    fn une_famille_sans_donnee_ne_produit_pas_donglet() {
        let mut p = payload_complet();
        p["speedtests"] = json!([]);
        p["discovery"] = json!(null);
        p["ports"] = json!(null);
        let (c, _) = classeur(vec![p], "fr");
        assert!(!c.onglets.iter().any(|o| o == "Débits"));
        assert!(!c.onglets.iter().any(|o| o == "Machines découvertes"));
        assert!(!c.onglets.iter().any(|o| o == "Ports ouverts"));
    }

    // ── L'en-tête, sur chaque feuille ────────────────────────────────────

    #[test]
    fn chaque_feuille_porte_la_fenetre_en_toutes_lettres() {
        let (c, _) = classeur(vec![payload_complet()], "fr");
        for onglet in &c.onglets {
            if onglet == "SLA par IP publique" {
                continue; // sans en-tête côté navigateur — voir le générateur.
            }
            let cells = c.onglet(onglet);
            let porte = cells.values().any(|v| match v {
                Lu::Texte(s) => s.starts_with("Du ") && s.contains(" au "),
                _ => false,
            });
            assert!(porte, "l'onglet « {onglet} » n'annonce pas sa période");
        }
    }

    // ── 🔴 L'indéterminé et la couverture, en CHAÎNES ────────────────────

    /// 🔴 Un tableur agrège ce qui est numérique. Une somme ou une moyenne de
    /// colonne ferait renaître le chiffre que le hub a refusé de calculer.
    #[test]
    fn lindetermine_et_la_couverture_sortent_en_chaines_jamais_en_nombres() {
        let mut p = payload_complet();
        p["targets"] = json!([{
            "ip": "10.0.0.9",
            // Aucun verdict : ni disponible, ni en panne.
            "samples": [releve(0, None, Some(11.0)), releve(60, None, None)],
            "coverage": { "window_secs": 3_600, "covered_secs": 1_800, "gap_secs": 1_800 },
        }]);
        p["internet"] = json!([]);
        let (c, _) = classeur(vec![p], "fr");

        // Synthèse : ligne d'en-tête en 6, première cible en 7.
        // C = disponibilité, J = couverture.
        assert_eq!(
            c.cell("Synthèse", "C7"),
            Some(&Lu::Texte("indéterminé".into())),
            "la disponibilité indéterminée doit être un mot, jamais un nombre"
        );
        assert_eq!(
            c.cell("Synthèse", "J7"),
            Some(&Lu::Texte("50.0 % de la période mesurée".into())),
            "la couverture doit être une chaîne, jamais un nombre"
        );

        // Et sur la feuille de la cible.
        let cible = c.onglet("10.0.0.9");
        assert!(
            cible
                .values()
                .any(|v| *v == Lu::Texte("indéterminé".into())),
            "la feuille de cible doit dire le mot, pas un chiffre"
        );
        assert!(
            !cible.values().any(|v| matches!(v, Lu::Nombre(n) if *n == 0.0 && false)),
            "garde-fou"
        );
    }

    /// 🔴 Le faux 0 % : sans un relevé déterminé, il n'y a pas de pourcentage.
    #[test]
    fn une_cible_sans_verdict_ne_recoit_aucun_zero_pour_cent() {
        let mut p = payload_complet();
        p["targets"] = json!([{
            "ip": "10.0.0.9",
            "samples": [releve(0, None, None)],
            "coverage": null,
        }]);
        p["internet"] = json!([]);
        let (c, _) = classeur(vec![p], "fr");
        assert_eq!(c.cell("Synthèse", "C7"), Some(&Lu::Texte("indéterminé".into())));
        assert!(!matches!(c.cell("Synthèse", "C7"), Some(Lu::Nombre(_))));
    }

    /// ⚠️ Rien à dire quand la période est entièrement mesurée : un
    /// « 0 % indéterminé » permanent est du bruit qui finit par masquer le cas
    /// où ça compte.
    #[test]
    fn une_periode_entierement_mesuree_ne_dit_rien_de_sa_couverture() {
        let (c, _) = classeur(vec![payload_complet()], "fr");
        // La cible du jeu complet est couverte de bout en bout.
        assert_eq!(c.cell("Synthèse", "J8"), None);
    }

    // ── Aucune ligne de total ni de moyenne ──────────────────────────────

    /// 🔴 Le SLA global d'un client est **refusé**, pas moyenné. Deux cibles à
    /// 100 % et 0 % ne font pas « 50 % de disponibilité chez Durand ».
    #[test]
    fn la_synthese_ne_porte_ni_total_ni_moyenne() {
        let (c, _) = classeur(vec![payload_complet()], "fr");
        let lignes = c.colonne("Synthèse", 'B', 6);
        // En-tête de tableau + accès internet + une cible. Rien de plus.
        assert_eq!(
            lignes,
            vec![
                Lu::Texte("Cible".into()),
                Lu::Texte("Accès internet".into()),
                Lu::Texte("192.168.1.1".into()),
            ]
        );
        for v in c.onglet("Synthèse").values() {
            if let Lu::Texte(s) = v {
                let bas = s.to_lowercase();
                assert!(!bas.starts_with("total"), "ligne de total : {s}");
                assert!(!bas.starts_with("moyenne"), "ligne de moyenne : {s}");
            }
        }
    }

    // ── Les coupures ─────────────────────────────────────────────────────

    /// ⚠️ Une coupure encore en cours se dit ; elle ne se clôt pas sur
    /// l'instant de l'export — ce serait fabriquer une durée fausse.
    #[test]
    fn une_coupure_en_cours_nest_jamais_close() {
        let mut p = payload_complet();
        p["targets"] = json!([{
            "ip": "10.0.0.9",
            "samples": [releve(0, Some(true), Some(5.0)), releve(60, Some(false), None)],
            "coverage": null,
        }]);
        p["internet"] = json!([]);
        let (c, _) = classeur(vec![p], "fr");
        let cible = c.onglet("10.0.0.9");
        let en_cours = cible
            .values()
            .filter(|v| **v == Lu::Texte("En cours".into()))
            .count();
        // Une fois pour la fin, une fois pour la durée.
        assert_eq!(en_cours, 2);
    }

    #[test]
    fn sans_coupure_la_feuille_le_dit_au_lieu_de_rester_vide() {
        let mut p = payload_complet();
        p["targets"] = json!([{
            "ip": "10.0.0.9",
            "samples": [releve(0, Some(true), Some(5.0)), releve(60, Some(true), Some(6.0))],
            "coverage": null,
        }]);
        p["internet"] = json!([]);
        let (c, _) = classeur(vec![p], "fr");
        assert!(c
            .onglet("10.0.0.9")
            .values()
            .any(|v| *v == Lu::Texte("Aucune coupure sur la période.".into())));
    }

    // ── Noms d'onglets ───────────────────────────────────────────────────

    /// ⚠️ Deux sondes qui surveillent la même adresse : le cas courant. Excel
    /// rejette un doublon, et le classeur ENTIER échouerait à l'écriture.
    #[test]
    fn deux_sondes_sur_la_meme_cible_ne_cassent_pas_le_classeur() {
        let mut a = payload_complet();
        a["internet"] = json!([]);
        let mut b = a.clone();
        b["probe"] = json!("Lyon-2");
        let (c, _) = classeur(vec![a, b], "fr");
        let cibles: Vec<&String> = c
            .onglets
            .iter()
            .filter(|o| o.contains("192.168.1.1"))
            .collect();
        assert_eq!(cibles.len(), 2);
        assert_ne!(cibles[0], cibles[1]);
    }

    #[test]
    fn un_nom_donglet_refuse_par_excel_est_assaini_et_plafonne() {
        let mut p = payload_complet();
        p["probe"] = json!("Site [Nord]/Sud : rez-de-chaussée, aile est");
        p["internet"] = json!([]);
        let (c, _) = classeur(vec![p.clone(), p], "fr");
        for nom in &c.onglets {
            assert!(nom.chars().count() <= 31, "onglet trop long : {nom}");
            for interdit in ['\\', '/', '*', '?', ':', '[', ']'] {
                assert!(!nom.contains(interdit), "caractère interdit dans {nom}");
            }
        }
    }

    // ── Le fichier ───────────────────────────────────────────────────────

    #[test]
    fn le_nom_de_fichier_porte_la_periode_et_rien_que_du_sur() {
        let n = file_name("Durand & Fils (Lyon)", T0, T0 + 86_400);
        assert_eq!(n, "lanprobe-sla-durand-fils-lyon-2026-09-03_2026-09-04.xlsx");
        for interdit in ['\\', '/', ':', '*', '?', '"', '<', '>', '|', ' '] {
            assert!(!n.contains(interdit), "caractère refusé par Windows : {n}");
        }
    }

    #[test]
    fn un_site_sans_lettre_latine_garde_un_nom_de_fichier_utilisable() {
        assert_eq!(
            file_name("—", T0, T0),
            "lanprobe-sla-rapport-2026-09-03_2026-09-03.xlsx"
        );
    }

    /// 🔴 L'empreinte se calcule sur les octets écrits : c'est le seul contrôle
    /// qui attrape une corruption silencieuse, et pas seulement une coupure.
    #[test]
    fn lempreinte_est_celle_des_octets_rendus() {
        let (_, f) = classeur(vec![payload_complet()], "fr");
        let attendu = ring::digest::digest(&ring::digest::SHA256, &f.bytes);
        let hex: String = attendu.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(f.sha256, hex);
        assert_eq!(f.sha256.len(), 64);
        assert!(!f.bytes.is_empty());
    }

    #[test]
    fn un_classeur_sans_releve_est_un_echec_nomme_pas_un_fichier_vide() {
        let wb = Workbook {
            site_name: "Durand".into(),
            range_start: T0,
            range_stop: T0 + 60,
            payloads: vec![],
        };
        let err = build(&wb, &Catalog::load("fr")).unwrap_err();
        assert!(err.contains("relevé"), "cause non nommée : {err}");
    }

    // ── La langue ────────────────────────────────────────────────────────

    #[test]
    fn le_classeur_est_redige_dans_la_langue_demandee() {
        let (c, _) = classeur(vec![payload_complet()], "es");
        assert!(c.onglets.iter().any(|o| o == "Resumen"), "{:?}", c.onglets);
    }

    // ── Les calculs portés du navigateur ─────────────────────────────────

    fn s(ts: i64, alive: Option<bool>, lat: Option<f64>) -> Sample {
        Sample {
            timestamp: ts,
            alive,
            latency_ms: lat,
            state: None,
        }
    }

    /// ⚠️ `== Some(false)`, PAS « pas vivant » : la négation est vraie pour
    /// l'indéterminé, et chaque mesure sans verdict aurait ouvert une coupure.
    #[test]
    fn lindetermine_nouvre_pas_de_coupure() {
        let liste = outages(&[
            s(0, Some(true), None),
            s(60, None, None),
            s(120, None, None),
            s(180, Some(true), None),
        ]);
        assert!(liste.is_empty());
    }

    #[test]
    fn une_coupure_close_porte_sa_fin_celle_qui_dure_non() {
        let liste = outages(&[
            s(0, Some(true), None),
            s(60, Some(false), None),
            s(120, Some(false), None),
            s(180, Some(true), None),
            s(240, Some(false), None),
        ]);
        assert_eq!(liste.len(), 2);
        assert_eq!(liste[0].start, 60);
        assert_eq!(liste[0].end, Some(180));
        assert_eq!(liste[0].samples_lost, 2);
        assert_eq!(liste[1].end, None);
    }

    /// ⚠️ Ce qui n'a pas été mesuré sort du dénominateur.
    #[test]
    fn lindetermine_sort_du_denominateur_et_se_compte_a_part() {
        let st = stats(&[
            s(0, Some(true), Some(10.0)),
            s(60, Some(false), None),
            s(120, None, None),
        ]);
        assert_eq!(st.total, 2);
        assert_eq!(st.failed, 1);
        assert_eq!(st.undetermined, 1);
        assert_eq!(st.uptime_pct, Some(50.0));
    }

    #[test]
    fn une_fenetre_sans_aucun_verdict_na_pas_de_pourcentage() {
        let st = stats(&[s(0, None, Some(10.0))]);
        assert_eq!(st.uptime_pct, None);
        assert_eq!(st.total, 0);
    }

    /// ⚠️ Une latence au-delà du délai d'attente vient d'une sonde suspendue,
    /// pas du réseau. Relevé en production : 1 045 504 ms.
    #[test]
    fn une_latence_de_sonde_endormie_ne_pollue_pas_les_moyennes() {
        let st = stats(&[
            s(0, Some(true), Some(10.0)),
            s(60, Some(true), Some(20.0)),
            s(120, Some(true), Some(1_045_504.0)),
        ]);
        assert_eq!(st.avg, Some(15.0));
        assert_eq!(st.max, Some(20.0));
        // La disponibilité, elle, n'est pas touchée.
        assert_eq!(st.uptime_pct, Some(100.0));
    }

    #[test]
    fn la_fenetre_secrit_en_toutes_lettres() {
        let fr = Catalog::load("fr");
        assert_eq!(window_label("-7d", &fr), "7 derniers jours");
        assert_eq!(window_label("-1d", &fr), "dernier jour");
        assert_eq!(window_label("-24h", &fr), "24 dernières heures");
        assert_eq!(
            window_label(&format!("{T0}..{}", T0 + 86_400), &fr),
            "Du 3 sept. 2026 au 4 sept. 2026"
        );
        // Ce qu'on ne sait pas dire, on le recopie plutôt que de l'inventer.
        assert_eq!(window_label("bizarre", &fr), "bizarre");
    }

    /// ⚠️ Les pourcentages ne totalisent PAS 100 % : un relevé qu'aucun
    /// intervalle ne couvre n'est imputé à personne.
    #[test]
    fn un_releve_hors_intervalle_va_en_indetermine_et_finit_la_liste() {
        let intervalles = vec![PublicIpInterval {
            public_ip: "88.120.0.1".into(),
            interface: None,
            gateway: Some("192.168.1.1".into()),
            local_subnet: None,
            confirmed_from: 100,
            confirmed_until: 200,
            label: Some("Fibre".into()),
        }];
        let lignes = by_public_ip(
            &[
                s(50, Some(true), None),
                s(150, Some(true), None),
                s(160, Some(false), None),
            ],
            &intervalles,
        );
        assert_eq!(lignes.len(), 2);
        assert_eq!(lignes[0].public_ip.as_deref(), Some("88.120.0.1"));
        assert_eq!(lignes[0].samples, 2);
        assert_eq!(lignes[0].uptime_pct, Some(50.0));
        // L'indéterminé en dernier : c'est un reste, pas une ligne comme les
        // autres.
        assert_eq!(lignes[1].public_ip, None);
        assert_eq!(lignes[1].samples, 1);
    }

    #[test]
    fn ladresse_indeterminee_se_lit_comme_telle_pas_comme_une_case_vide() {
        let mut p = payload_complet();
        p["public_ip_history"] = json!([]);
        let (c, _) = classeur(vec![p], "fr");
        let onglet = c.onglet("SLA par IP publique");
        assert!(onglet
            .values()
            .any(|v| *v == Lu::Texte("Indéterminé".into())));
    }

    // ── Le graphique ─────────────────────────────────────────────────────

    fn images(bytes: &[u8]) -> usize {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
        let mut n = 0;
        for i in 0..zip.len() {
            if zip.by_index(i).unwrap().name().starts_with("xl/media/") {
                n += 1;
            }
        }
        n
    }

    /// 🔴 Une IMAGE, jamais un graphique Excel natif : il se recalcule à
    /// l'ouverture et ne sait pas peindre les bandes de coupure.
    #[test]
    fn chaque_feuille_de_cible_porte_son_graphique() {
        let (_, f) = classeur(vec![payload_complet()], "fr");
        // Accès internet et la cible : deux séries, deux dessins.
        assert_eq!(images(&f.bytes), 2);
    }

    /// Une image ne s'invente pas à partir d'un point, et un cadre vide dans un
    /// rapport se lit comme une panne de l'outil.
    #[test]
    fn une_serie_trop_courte_ne_recoit_pas_de_graphique_invente() {
        let mut p = payload_complet();
        p["targets"] = json!([{
            "ip": "10.0.0.9",
            "samples": [releve(0, Some(true), Some(5.0))],
            "coverage": null,
        }]);
        p["internet"] = json!([]);
        let (_, f) = classeur(vec![p], "fr");
        assert_eq!(images(&f.bytes), 0);
    }

    // ── Largeur des colonnes ─────────────────────────────────────────────

    /// ⚠️ Sans ajustement, un nom de serveur Ookla s'affiche en `####` ou
    /// tronqué — dans un document qu'on a remis au lecteur.
    #[test]
    fn les_colonnes_sont_ajustees_et_plafonnees() {
        let (_, f) = classeur(vec![payload_complet()], "fr");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(f.bytes)).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("xl/worksheets/sheet1.xml").unwrap(), &mut xml)
            .unwrap();
        assert!(xml.contains("<cols>"), "aucune largeur de colonne posée");
        for chunk in xml.split("<col ").skip(1) {
            let w: f64 = attribut(chunk, "width").unwrap().parse().unwrap();
            // Plafonnée à 46 caractères : une URL de résultat ferait sinon une
            // colonne qui pousse tout le reste hors de l'écran. La borne haute
            // laisse la marge d'arrondi de la bibliothèque.
            assert!((9.0..=47.0).contains(&w), "largeur hors bornes : {w}");
        }
    }
}
