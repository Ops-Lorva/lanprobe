//! Le graphique de latence d'une cible, en PNG (contrat § 23).
//!
//! 🔴 **Une IMAGE, jamais un graphique Excel natif.** Tranché par Benjamin, et
//! pour deux raisons distinctes :
//!
//! 1. Un graphique natif **se recalcule à l'ouverture**. Le client trie une
//!    colonne de la feuille et la courbe bouge sous ses yeux. Une image dit ce
//!    qu'on a mesuré, définitivement.
//! 2. Surtout : un graphique natif **ne sait pas peindre les bandes de
//!    coupure**. Il relierait deux mesures séparées par une panne d'une belle
//!    droite continue — la faute cardinale de ce produit, une valeur plausible
//!    et fausse à l'endroit exact où elle coûte le plus cher.
//!
//! ⚠️ Les coupures sont peintes **en fond**, pas seulement absentes de la
//! courbe : une ligne qui s'interrompt se confond avec une ligne qui sort du
//! cadre.
//!
//! ⚠️ **Le poids de la police est assumé, côté serveur.** Le dessin porte des
//! graduations, un axe des temps et un titre ; sans vraie police il n'y a pas
//! de texte, et une courbe sans axe des temps ne dit que « ça a bougé ». Les
//! 750 ko de DejaVu Sans partent dans le binaire du **hub**, dont l'image
//! Docker se tire une fois — l'argument du binaire qu'on veut petit vaut pour
//! la SONDE, qu'on installe chez le client.

// Le générateur n'a pas encore d'appelant hors du classeur.
#![allow(dead_code)]

use crate::report_i18n::Catalog;
use crate::report_xlsx::{outages, Sample};

/// Dimensions du dessin, reprises du générateur du navigateur.
pub(crate) const LARGEUR: usize = 900;
pub(crate) const HAUTEUR: usize = 260;

const PAD_HAUT: usize = 16;
const PAD_DROITE: usize = 16;
const PAD_BAS: usize = 26;
const PAD_GAUCHE: usize = 52;

/// Au-delà de 36 h, l'heure seule devient ambiguë : deux points à 14:00 peuvent
/// être à deux jours d'écart.
const SEUIL_DATE_SUR_AXE: i64 = 36 * 3_600;

const BLANC: [u8; 3] = [0xff, 0xff, 0xff];
const COUPURE: [u8; 3] = [0xef, 0x44, 0x44];
const COUPURE_ALPHA: f32 = 0.16;
const GRILLE: [u8; 3] = [0xe5, 0xe7, 0xeb];
const GRADUATION: [u8; 3] = [0x6b, 0x72, 0x80];
const COURBE: [u8; 3] = [0x4f, 0x46, 0xe5];
const TITRE: [u8; 3] = [0x37, 0x41, 0x51];

const TAILLE_TEXTE: f32 = 11.0;

/// La police du dessin, embarquée dans le binaire.
///
/// ⚠️ Pas la police du système : un conteneur n'en a aucune, et un graphique
/// dont les graduations disparaissent selon l'image de base est pire qu'un
/// graphique sans graduations — il en a eu, une fois.
const POLICE: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

/// Le graphique de latence d'une cible, en PNG.
///
/// `None` quand il n'y a rien à tracer : une image ne s'invente pas à partir
/// d'un point, et un cadre vide dans un rapport se lit comme une panne de
/// l'outil.
pub(crate) fn chart_png(samples: &[Sample], c: &Catalog) -> Option<Vec<u8>> {
    if samples.len() < 2 {
        return None;
    }
    let t0 = samples[0].timestamp;
    let t1 = samples[samples.len() - 1].timestamp;
    let span = (t1 - t0).max(1);

    let latences: Vec<f64> = samples
        .iter()
        .filter_map(|s| s.latency_ms)
        .filter(|v| v.is_finite())
        .collect();
    let v_max = latences.iter().cloned().fold(1.0_f64, f64::max) * 1.15;

    let plot_l = (LARGEUR - PAD_GAUCHE - PAD_DROITE) as f64;
    let plot_h = (HAUTEUR - PAD_HAUT - PAD_BAS) as f64;
    let x = |ts: i64| PAD_GAUCHE as f64 + (ts - t0) as f64 / span as f64 * plot_l;
    let y = |v: f64| PAD_HAUT as f64 + plot_h - (v / v_max) * plot_h;

    let mut toile = Toile::blanche(LARGEUR, HAUTEUR);
    let police = fontdue::Font::from_bytes(POLICE, fontdue::FontSettings::default()).ok()?;

    // ── Coupures en fond ────────────────────────────────────────────────
    //
    // 🔴 Peintes, pas seulement absentes de la courbe : une ligne qui
    // s'interrompt se confond avec une ligne qui sort du cadre.
    for o in outages(samples) {
        let de = x(o.start);
        let a = x(o.end.unwrap_or(t1));
        toile.rectangle(
            de,
            PAD_HAUT as f64,
            (a - de).max(1.0),
            plot_h,
            COUPURE,
            COUPURE_ALPHA,
        );
    }

    // ── Grille et graduations ───────────────────────────────────────────
    for i in 0..=4 {
        let v = v_max / 4.0 * i as f64;
        let yy = y(v).round();
        toile.horizontale(PAD_GAUCHE as f64, (LARGEUR - PAD_DROITE) as f64, yy, GRILLE);
        toile.texte(
            &police,
            &format!("{v:.0}"),
            6.0,
            yy + 4.0,
            Alignement::Gauche,
            GRADUATION,
        );
    }

    // ── La courbe, coupée sur les interruptions ─────────────────────────
    //
    // ⚠️ Une droite au travers d'un trou affirmerait une latence pendant une
    // période sans mesure.
    let mut precedent: Option<(f64, f64)> = None;
    for s in samples {
        let (Some(true), Some(lat)) = (s.alive, s.latency_ms) else {
            precedent = None;
            continue;
        };
        let point = (x(s.timestamp), y(lat));
        if let Some(depuis) = precedent {
            toile.ligne(depuis, point, COURBE);
        }
        precedent = Some(point);
    }

    // ── L'axe des temps ─────────────────────────────────────────────────
    //
    // ⚠️ Sans lui, une courbe ne dit que « ça a bougé » : on ne sait ni quand la
    // coupure a eu lieu, ni combien de temps couvre le graphique. Dans un
    // rapport remis à un client, c'est précisément la question qu'il posera.
    const GRADUATIONS: i64 = 5;
    for i in 0..GRADUATIONS {
        let ts = t0 + span * i / (GRADUATIONS - 1);
        let px = x(ts)
            .max(PAD_GAUCHE as f64 + 30.0)
            .min((LARGEUR - PAD_DROITE) as f64 - 30.0);
        toile.texte(
            &police,
            &instant_sur_axe(ts, span),
            px,
            (HAUTEUR - 8) as f64,
            Alignement::Centre,
            GRADUATION,
        );
    }

    toile.texte(
        &police,
        &c.t("sla.col_latency", &[]),
        PAD_GAUCHE as f64,
        11.0,
        Alignement::Gauche,
        TITRE,
    );

    toile.encoder()
}

/// L'instant tel qu'il s'écrit sur l'axe, dans le fuseau du hub.
///
/// ⚠️ La date apparaît dès que la fenêtre dépasse [`SEUIL_DATE_SUR_AXE`] :
/// au-delà, deux points à 14:00 peuvent être à deux jours d'écart.
fn instant_sur_axe(epoch_secs: i64, span: i64) -> String {
    let (_, mois, jour, hh, mm, _) = crate::report_i18n::civil_from_epoch(epoch_secs);
    if span > SEUIL_DATE_SUR_AXE {
        format!("{jour:02}/{mois:02} {hh:02}:{mm:02}")
    } else {
        format!("{hh:02}:{mm:02}")
    }
}

enum Alignement {
    Gauche,
    Centre,
}

/// Une image RVB en mémoire, et le strict nécessaire pour y dessiner.
///
/// ⚠️ Pas de bibliothèque de rendu : le dessin tient en quatre primitives —
/// rectangle, ligne, horizontale, texte. En tirer une de plus pour ça
/// coûterait davantage à maintenir que ces quatre-là.
struct Toile {
    largeur: usize,
    hauteur: usize,
    pixels: Vec<u8>,
}

impl Toile {
    fn blanche(largeur: usize, hauteur: usize) -> Self {
        Self {
            largeur,
            hauteur,
            pixels: vec![0xff; largeur * hauteur * 3],
        }
    }

    /// Pose une couleur avec sa couverture, sur ce qui est déjà là.
    fn melanger(&mut self, x: i64, y: i64, couleur: [u8; 3], alpha: f32) {
        if x < 0 || y < 0 || x >= self.largeur as i64 || y >= self.hauteur as i64 || alpha <= 0.0 {
            return;
        }
        let alpha = alpha.min(1.0);
        let i = (y as usize * self.largeur + x as usize) * 3;
        for canal in 0..3 {
            let fond = self.pixels[i + canal] as f32;
            let dessus = couleur[canal] as f32;
            self.pixels[i + canal] = (fond + (dessus - fond) * alpha).round() as u8;
        }
    }

    fn rectangle(&mut self, x: f64, y: f64, l: f64, h: f64, couleur: [u8; 3], alpha: f32) {
        let x0 = x.round() as i64;
        let y0 = y.round() as i64;
        let x1 = (x + l).round() as i64;
        let y1 = (y + h).round() as i64;
        for yy in y0..y1 {
            for xx in x0..x1 {
                self.melanger(xx, yy, couleur, alpha);
            }
        }
    }

    fn horizontale(&mut self, x0: f64, x1: f64, y: f64, couleur: [u8; 3]) {
        let yy = y.round() as i64;
        for xx in (x0.round() as i64)..(x1.round() as i64) {
            self.melanger(xx, yy, couleur, 1.0);
        }
    }

    /// Un segment, épais de deux pixels pour qu'il se voie à l'impression.
    fn ligne(&mut self, de: (f64, f64), a: (f64, f64), couleur: [u8; 3]) {
        let dx = a.0 - de.0;
        let dy = a.1 - de.1;
        let pas = dx.abs().max(dy.abs()).max(1.0).ceil() as i64;
        for i in 0..=pas {
            let f = i as f64 / pas as f64;
            let x = (de.0 + dx * f).round() as i64;
            let y = (de.1 + dy * f).round() as i64;
            self.melanger(x, y, couleur, 1.0);
            // L'épaisseur se pose perpendiculairement à la pente : un trait
            // épaissi toujours vers le bas se décale visiblement sur les
            // segments raides.
            if dx.abs() >= dy.abs() {
                self.melanger(x, y + 1, couleur, 1.0);
            } else {
                self.melanger(x + 1, y, couleur, 1.0);
            }
        }
    }

    /// Écrit une ligne de texte, `y` étant la **ligne de base**.
    fn texte(
        &mut self,
        police: &fontdue::Font,
        texte: &str,
        x: f64,
        y: f64,
        alignement: Alignement,
        couleur: [u8; 3],
    ) {
        let largeur: f32 = texte
            .chars()
            .map(|c| police.metrics(c, TAILLE_TEXTE).advance_width)
            .sum();
        let mut plume = match alignement {
            Alignement::Gauche => x as f32,
            Alignement::Centre => x as f32 - largeur / 2.0,
        };
        for c in texte.chars() {
            let (m, bitmap) = police.rasterize(c, TAILLE_TEXTE);
            // `ymin` est le bas du dessin par rapport à la ligne de base ; le
            // haut s'en déduit, sans quoi accents et jambages se posent de
            // travers.
            let haut = y as i64 - (m.height as i64 + m.ymin as i64);
            let gauche = plume.round() as i64 + m.xmin as i64;
            for (i, couverture) in bitmap.iter().enumerate() {
                if *couverture == 0 {
                    continue;
                }
                let (dx, dy) = (i % m.width, i / m.width);
                self.melanger(
                    gauche + dx as i64,
                    haut + dy as i64,
                    couleur,
                    *couverture as f32 / 255.0,
                );
            }
            plume += m.advance_width;
        }
    }

    fn encoder(&self) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        {
            let mut encodeur =
                png::Encoder::new(&mut out, self.largeur as u32, self.hauteur as u32);
            encodeur.set_color(png::ColorType::Rgb);
            encodeur.set_depth(png::BitDepth::Eight);
            let mut ecriture = encodeur.write_header().ok()?;
            ecriture.write_image_data(&self.pixels).ok()?;
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(ts: i64, alive: Option<bool>, lat: Option<f64>) -> Sample {
        Sample {
            timestamp: ts,
            alive,
            latency_ms: lat,
            state: None,
        }
    }

    fn decoder(png: &[u8]) -> (u32, u32, Vec<u8>) {
        let decodeur = png::Decoder::new(std::io::Cursor::new(png));
        let mut lecteur = decodeur.read_info().expect("PNG illisible");
        let mut buf = vec![0; lecteur.output_buffer_size().unwrap()];
        let info = lecteur.next_frame(&mut buf).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgb);
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

    fn pixel(px: &[u8], l: u32, x: usize, y: usize) -> [u8; 3] {
        let i = (y * l as usize + x) * 3;
        [px[i], px[i + 1], px[i + 2]]
    }

    /// Une image ne s'invente pas à partir d'un point.
    #[test]
    fn moins_de_deux_releves_ne_produit_pas_dimage() {
        let c = Catalog::load("fr");
        assert!(chart_png(&[], &c).is_none());
        assert!(chart_png(&[s(0, Some(true), Some(10.0))], &c).is_none());
    }

    #[test]
    fn limage_est_un_png_aux_dimensions_du_generateur_du_navigateur() {
        let c = Catalog::load("fr");
        let samples: Vec<Sample> = (0..60)
            .map(|i| s(1_788_436_800 + i * 60, Some(true), Some(10.0 + i as f64)))
            .collect();
        let png = chart_png(&samples, &c).expect("une image");
        assert_eq!(&png[1..4], b"PNG");
        let (l, h, _) = decoder(&png);
        assert_eq!((l as usize, h as usize), (LARGEUR, HAUTEUR));
    }

    /// 🔴 Le test qui justifie l'image plutôt qu'un graphique Excel natif : la
    /// coupure est **peinte**, pas seulement absente de la courbe.
    #[test]
    fn une_coupure_est_peinte_en_fond() {
        let c = Catalog::load("fr");
        let t0 = 1_788_436_800;
        // Vingt relevés : les cinq du milieu en panne.
        let samples: Vec<Sample> = (0..20)
            .map(|i| {
                if (8..13).contains(&i) {
                    s(t0 + i * 60, Some(false), None)
                } else {
                    s(t0 + i * 60, Some(true), Some(20.0))
                }
            })
            .collect();
        let png = chart_png(&samples, &c).expect("une image");
        let (l, _, px) = decoder(&png);

        // Au milieu de la coupure, le fond est teinté de rouge.
        let x_coupure = PAD_GAUCHE + (LARGEUR - PAD_GAUCHE - PAD_DROITE) / 2;
        let dans = pixel(&px, l, x_coupure, PAD_HAUT + 5);
        assert!(
            dans[0] > dans[2] + 10,
            "la bande de coupure n'est pas peinte : {dans:?}"
        );

        // Hors de la coupure, le fond reste blanc.
        let hors = pixel(&px, l, PAD_GAUCHE + 4, PAD_HAUT + 5);
        assert_eq!(hors, BLANC, "le fond hors coupure devrait rester blanc");
    }

    /// ⚠️ Une droite au travers d'un trou affirmerait une latence pendant une
    /// période sans mesure.
    #[test]
    fn la_courbe_ne_traverse_pas_la_coupure() {
        let c = Catalog::load("fr");
        let t0 = 1_788_436_800;
        // Avant la coupure : tout en bas. Après : tout en haut. Une droite
        // reliant les deux traverserait le milieu du cadre.
        let mut samples = Vec::new();
        for i in 0..8 {
            samples.push(s(t0 + i * 60, Some(true), Some(1.0)));
        }
        for i in 8..13 {
            samples.push(s(t0 + i * 60, Some(false), None));
        }
        for i in 13..20 {
            samples.push(s(t0 + i * 60, Some(true), Some(900.0)));
        }
        let png = chart_png(&samples, &c).expect("une image");
        let (l, _, px) = decoder(&png);

        let plot_l = LARGEUR - PAD_GAUCHE - PAD_DROITE;
        let debut = PAD_GAUCHE + plot_l * 9 / 19;
        let fin = PAD_GAUCHE + plot_l * 12 / 19;
        for x in debut..fin {
            for y in PAD_HAUT..(HAUTEUR - PAD_BAS) {
                let p = pixel(&px, l, x, y);
                assert_ne!(
                    p, COURBE,
                    "la courbe traverse la coupure en ({x}, {y}) — elle affirmerait \
                     une latence pendant une période sans mesure"
                );
            }
        }
    }

    /// ⚠️ Sans axe des temps, une courbe ne dit que « ça a bougé » : on ne sait
    /// ni quand la coupure a eu lieu, ni combien de temps couvre le graphique.
    /// Dans un rapport remis à un client, c'est la question qu'il posera.
    #[test]
    fn laxe_des_temps_et_les_graduations_sont_ecrits() {
        let c = Catalog::load("fr");
        let t0 = 1_788_436_800;
        let samples: Vec<Sample> = (0..20)
            .map(|i| s(t0 + i * 60, Some(true), Some(20.0)))
            .collect();
        let png = chart_png(&samples, &c).expect("une image");
        let (l, _, px) = decoder(&png);

        // Le bandeau du bas porte du texte : des pixels non blancs sous l'aire
        // de tracé.
        let encre_bas = (0..LARGEUR)
            .flat_map(|x| ((HAUTEUR - PAD_BAS + 2)..HAUTEUR).map(move |y| (x, y)))
            .filter(|(x, y)| pixel(&px, l, *x, *y) != BLANC)
            .count();
        assert!(encre_bas > 50, "aucun axe des temps : {encre_bas} pixels");

        // La colonne de gauche porte les graduations de latence.
        let encre_gauche = (0..PAD_GAUCHE)
            .flat_map(|x| (0..HAUTEUR).map(move |y| (x, y)))
            .filter(|(x, y)| pixel(&px, l, *x, *y) != BLANC)
            .count();
        assert!(
            encre_gauche > 50,
            "aucune graduation de latence : {encre_gauche} pixels"
        );
    }

    /// La date apparaît sur l'axe dès que la fenêtre dépasse 36 h : deux points
    /// à 14:00 peuvent être à deux jours d'écart.
    #[test]
    fn au_dela_de_trente_six_heures_laxe_porte_la_date() {
        assert_eq!(instant_sur_axe(1_788_436_800, 3_600), "12:00");
        assert_eq!(
            instant_sur_axe(1_788_436_800, 4 * 86_400),
            "03/09 12:00"
        );
    }
}
