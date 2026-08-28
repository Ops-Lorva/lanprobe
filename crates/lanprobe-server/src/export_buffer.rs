//! Tampon des points de mesure en attente d'écriture.
//!
//! Le tampon est la mémoire courte de la sonde entre deux écritures réussies.
//! Il ne se vide **qu'après confirmation** de l'écriture : perdre les points
//! parce que la destination ne répond pas, c'est perdre la trace de
//! l'incident qu'on voudrait analyser.

use std::collections::VecDeque;

/// Un point déjà rendu en Line Protocol, avec l'horodatage **de la mesure**.
/// Le rejeu réutilise cet horodatage : un point réémis doit retomber au bon
/// endroit dans le graphe, pas à l'heure de l'envoi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferedPoint {
    pub ts_ns: u64,
    pub line: String,
}

/// Ce qu'une insertion a fait abandonner. Un tampon qui déborde en silence
/// ment sur la complétude des données : le compte exact est remonté à
/// l'appelant *et* journalisé.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dropped {
    /// Points jetés parce que le tampon a dépassé sa capacité.
    pub too_many: usize,
    /// Points jetés parce qu'ils dépassaient l'âge maximum.
    pub too_old: usize,
}

impl Dropped {
    pub fn any(&self) -> bool {
        self.too_many > 0 || self.too_old > 0
    }
}

/// Plafond du tampon. Au-delà, les points les plus anciens sont abandonnés
/// — et comptés.
pub const MAX_POINTS: usize = 100_000;
/// Âge maximum d'un point en attente. Un point vieux de plus d'un jour ne
/// décrit plus l'incident qu'on analyse.
pub const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

pub struct ExportBuffer {
    points: VecDeque<BufferedPoint>,
    max_points: usize,
    max_age_ns: u64,
    /// `None` = tampon purement en mémoire (tests, et sonde sans répertoire
    /// de config accessible).
    path: Option<std::path::PathBuf>,
    dirty: bool,
}

impl ExportBuffer {
    /// Tampon sans persistance, aux bornes par défaut.
    pub fn new() -> Self {
        Self::with_limits(MAX_POINTS, MAX_AGE)
    }

    /// Bornes explicites — utilisé par les tests, qui n'ont pas 24 h.
    pub fn with_limits(max_points: usize, max_age: std::time::Duration) -> Self {
        Self {
            points: VecDeque::new(),
            max_points,
            max_age_ns: max_age.as_nanos().min(u64::MAX as u128) as u64,
            path: None,
            dirty: false,
        }
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Ajoute une ligne Line Protocol. `now_ns` sert uniquement à mesurer
    /// l'âge des points déjà en attente.
    pub fn push_at(&mut self, line: String, now_ns: u64) -> Dropped {
        let ts_ns = timestamp_of(&line).unwrap_or(now_ns);
        self.points.push_back(BufferedPoint { ts_ns, line });
        self.dirty = true;
        self.enforce_bounds(now_ns)
    }

    /// Le lot prêt à partir : le nombre de points et le corps de la requête.
    pub fn batch(&self) -> Option<(usize, String)> {
        if self.points.is_empty() {
            return None;
        }
        let body = self
            .points
            .iter()
            .map(|p| p.line.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Some((self.points.len(), body))
    }

    /// Confirme l'écriture des `count` points les plus anciens — eux seuls
    /// quittent le tampon. Les points arrivés pendant l'écriture restent.
    pub fn confirm(&mut self, count: usize) {
        let count = count.min(self.points.len());
        if count > 0 {
            self.points.drain(0..count);
            self.dirty = true;
        }
    }

    /// Chemin du tampon persistant dans le répertoire de config.
    pub fn default_path(config_dir: &std::path::Path) -> std::path::PathBuf {
        config_dir.join("buffer.ndjson")
    }

    /// Ouvre un tampon adossé à `path`, en relisant ce qu'une exécution
    /// précédente y a laissé. Une coupure de courant pendant l'incident ne
    /// doit pas effacer la trace de l'incident.
    pub fn open_at(path: std::path::PathBuf, now_ns: u64) -> Self {
        Self::open_with_limits_at(path, MAX_POINTS, MAX_AGE, now_ns)
    }

    pub fn open_with_limits_at(
        path: std::path::PathBuf,
        max_points: usize,
        max_age: std::time::Duration,
        now_ns: u64,
    ) -> Self {
        let mut buf = Self::with_limits(max_points, max_age);
        buf.points = read_ndjson(&path);
        buf.path = Some(path);
        let restored = buf.points.len();
        let dropped = buf.enforce_bounds(now_ns);
        // Le rechargement ne salit le tampon que s'il en a jeté : sinon le
        // fichier décrit déjà exactement ce qu'on a en mémoire.
        buf.dirty = dropped.any();
        if restored > 0 {
            tracing::info!(
                "tampon d'export : {} points repris du disque ({} abandonnés)",
                buf.points.len(),
                dropped.too_old + dropped.too_many
            );
        }
        buf
    }

    /// `true` quand le contenu en mémoire diffère du fichier.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Réécrit le fichier — atomiquement, pour ne jamais laisser un tampon
    /// tronqué derrière un arrêt brutal.
    pub fn persist(&mut self) -> Result<(), String> {
        let Some(path) = self.path.clone() else {
            self.dirty = false;
            return Ok(());
        };
        let mut body = String::new();
        for p in &self.points {
            body.push_str(&serde_json::json!({ "ts_ns": p.ts_ns, "line": p.line }).to_string());
            body.push('\n');
        }
        let tmp = path.with_extension("ndjson.tmp");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        self.dirty = false;
        Ok(())
    }

    /// Abandonne tout ce qui est en attente. Réservé au cas où l'utilisateur
    /// coupe lui-même l'export : une panne, elle, ne fait jamais oublier.
    pub fn discard_all(&mut self) {
        if !self.points.is_empty() {
            tracing::info!(
                "export désactivé : {} points en attente abandonnés",
                self.points.len()
            );
        }
        self.points.clear();
        self.dirty = true;
    }

    /// Applique les deux bornes et journalise ce qui a été abandonné.
    fn enforce_bounds(&mut self, now_ns: u64) -> Dropped {
        let mut dropped = Dropped::default();

        let cutoff = now_ns.saturating_sub(self.max_age_ns);
        while let Some(front) = self.points.front() {
            if front.ts_ns < cutoff {
                self.points.pop_front();
                dropped.too_old += 1;
            } else {
                break;
            }
        }
        if self.points.len() > self.max_points {
            dropped.too_many = self.points.len() - self.max_points;
            self.points.drain(0..dropped.too_many);
        }

        if dropped.too_old > 0 {
            tracing::warn!(
                "tampon d'export : {} points au-delà de {} h abandonnés",
                dropped.too_old,
                self.max_age_ns / 3_600_000_000_000
            );
        }
        if dropped.too_many > 0 {
            tracing::warn!(
                "tampon d'export plein ({} points) : {} points les plus anciens abandonnés",
                self.max_points,
                dropped.too_many
            );
        }
        dropped
    }
}

impl Default for ExportBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Lit l'horodatage en fin de ligne Line Protocol (`… champs <ts_ns>`).
fn timestamp_of(line: &str) -> Option<u64> {
    line.rsplit(' ').next()?.parse().ok()
}

/// Relit le fichier de tampon. Une ligne illisible est ignorée : mieux vaut
/// rejouer ce qui reste lisible que refuser de démarrer.
fn read_ndjson(path: &std::path::Path) -> VecDeque<BufferedPoint> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return VecDeque::new();
    };
    let mut points = VecDeque::new();
    let mut illisibles = 0usize;
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => match (v["ts_ns"].as_u64(), v["line"].as_str()) {
                (Some(ts_ns), Some(l)) => points.push_back(BufferedPoint {
                    ts_ns,
                    line: l.to_string(),
                }),
                _ => illisibles += 1,
            },
            Err(_) => illisibles += 1,
        }
    }
    if illisibles > 0 {
        tracing::warn!("tampon d'export : {illisibles} lignes illisibles ignorées à la relecture");
    }
    points
}

// ── Repli ──────────────────────────────────────────────────────────────────

/// Délai après le premier échec.
pub const BACKOFF_BASE: std::time::Duration = std::time::Duration::from_secs(1);
/// Plafond du repli : au pire une tentative par minute. Retenter chaque
/// seconde contre une destination morte n'aide personne et remplit les logs.
pub const BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(60);

/// Repli exponentiel plafonné entre deux tentatives d'écriture.
#[derive(Debug, Default)]
pub struct Backoff {
    delay: std::time::Duration,
    next_attempt: Option<std::time::Instant>,
}

impl Backoff {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn delay(&self) -> std::time::Duration {
        self.delay
    }

    /// `true` quand l'échéance du repli est passée.
    pub fn ready_at(&self, now: std::time::Instant) -> bool {
        match self.next_attempt {
            Some(t) => now >= t,
            None => true,
        }
    }

    /// Enregistre un échec et rend le nouveau délai.
    pub fn record_failure_at(&mut self, now: std::time::Instant) -> std::time::Duration {
        self.delay = if self.delay.is_zero() {
            BACKOFF_BASE
        } else {
            (self.delay * 2).min(BACKOFF_MAX)
        };
        self.next_attempt = Some(now + self.delay);
        self.delay
    }

    /// Une écriture a réussi : on repart à la cadence nominale.
    pub fn reset(&mut self) {
        self.delay = std::time::Duration::ZERO;
        self.next_attempt = None;
    }
}

// ── Écriture ───────────────────────────────────────────────────────────────

/// Destination d'un lot de points. Abstrait InfluxDB et le hub, et permet de
/// tester la boucle d'envoi sans réseau.
pub trait PointWriter {
    fn write_points(&self, body: String) -> impl Future<Output = Result<(), String>> + Send;
}

#[derive(Debug, PartialEq, Eq)]
pub enum FlushOutcome {
    /// Rien à envoyer, ou repli en cours : la destination n'a pas été sollicitée.
    Skipped,
    /// `n` points confirmés et retirés du tampon.
    Written(usize),
    /// Échec : le tampon garde tout, le repli s'allonge.
    Failed(String),
}

/// Une tentative d'écriture. Le tampon ne perd un point qu'une fois la
/// destination confirmée — jamais avant.
pub async fn flush_once<W: PointWriter>(
    writer: &W,
    buffer: &mut ExportBuffer,
    backoff: &mut Backoff,
    now: std::time::Instant,
) -> FlushOutcome {
    if !backoff.ready_at(now) {
        return FlushOutcome::Skipped;
    }
    let Some((count, body)) = buffer.batch() else {
        return FlushOutcome::Skipped;
    };
    match writer.write_points(body).await {
        Ok(()) => {
            buffer.confirm(count);
            backoff.reset();
            FlushOutcome::Written(count)
        }
        Err(e) => {
            let delay = backoff.record_failure_at(now);
            tracing::warn!(
                "écriture des mesures refusée ({e}) — {} points conservés, prochaine tentative dans {} s",
                buffer.len(),
                delay.as_secs()
            );
            FlushOutcome::Failed(e)
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const SEC: u64 = 1_000_000_000;

    fn point(ts_ns: u64) -> String {
        format!("ping_latency,host=a,ip=1.2.3.4 alive=true {ts_ns}")
    }

    #[test]
    fn un_lot_non_confirme_reste_dans_le_tampon() {
        let mut buf = ExportBuffer::new();
        buf.push_at(point(1 * SEC), 1 * SEC);
        buf.push_at(point(2 * SEC), 2 * SEC);
        let (count, _body) = buf.batch().expect("un lot est prêt");
        assert_eq!(count, 2);
        // L'écriture a échoué : on ne confirme rien.
        assert_eq!(buf.len(), 2, "aucun point ne doit être perdu");
    }

    #[test]
    fn seuls_les_points_confirmes_quittent_le_tampon() {
        let mut buf = ExportBuffer::new();
        buf.push_at(point(1 * SEC), 1 * SEC);
        buf.push_at(point(2 * SEC), 2 * SEC);
        let (count, _) = buf.batch().unwrap();
        // Un point arrive pendant l'écriture.
        buf.push_at(point(3 * SEC), 3 * SEC);
        buf.confirm(count);
        assert_eq!(buf.len(), 1);
        let (_, body) = buf.batch().unwrap();
        assert_eq!(body, point(3 * SEC));
    }

    #[test]
    fn le_lot_conserve_l_horodatage_de_la_mesure() {
        let mut buf = ExportBuffer::new();
        buf.push_at(point(7 * SEC), 7 * SEC);
        // Le rejeu a lieu bien plus tard : la ligne ne doit pas être re-datée.
        let (_, body) = buf.batch().unwrap();
        assert_eq!(body, point(7 * SEC));
    }

    #[test]
    fn le_debordement_jette_les_plus_anciens_et_les_compte() {
        let mut buf = ExportBuffer::with_limits(3, Duration::from_secs(3600));
        for i in 1..=3 {
            assert_eq!(buf.push_at(point(i * SEC), i * SEC), Dropped::default());
        }
        let dropped = buf.push_at(point(4 * SEC), 4 * SEC);
        assert_eq!(dropped.too_many, 1);
        assert_eq!(buf.len(), 3);
        let (_, body) = buf.batch().unwrap();
        assert!(
            body.starts_with(&point(2 * SEC)),
            "le plus ancien doit partir en premier, pas le plus récent : {body}"
        );
    }

    #[test]
    fn les_points_trop_vieux_sont_jetes_et_comptes() {
        let mut buf = ExportBuffer::with_limits(100, Duration::from_secs(10));
        buf.push_at(point(1 * SEC), 1 * SEC);
        buf.push_at(point(2 * SEC), 2 * SEC);
        // 30 s plus tard : les deux premiers ont dépassé les 10 s.
        let dropped = buf.push_at(point(30 * SEC), 30 * SEC);
        assert_eq!(dropped.too_old, 2);
        assert_eq!(buf.len(), 1);
    }

    fn tmp_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lanprobe-buffer-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("buffer.ndjson")
    }

    #[test]
    fn le_tampon_est_relu_au_redemarrage() {
        let path = tmp_path("relu");
        let _ = std::fs::remove_file(&path);
        let mut buf = ExportBuffer::open_at(path.clone(), 10 * SEC);
        buf.push_at(point(9 * SEC), 10 * SEC);
        buf.push_at(point(10 * SEC), 10 * SEC);
        buf.persist().unwrap();

        // Redémarrage : rien n'a été écrit vers Influx entre-temps.
        let relu = ExportBuffer::open_at(path, 11 * SEC);
        assert_eq!(relu.len(), 2);
        let (_, body) = relu.batch().unwrap();
        assert_eq!(body, format!("{}\n{}", point(9 * SEC), point(10 * SEC)));
    }

    #[test]
    fn le_rejeu_apres_redemarrage_garde_l_horodatage_de_la_mesure() {
        let path = tmp_path("horodatage");
        let _ = std::fs::remove_file(&path);
        let mut buf = ExportBuffer::open_at(path.clone(), 5 * SEC);
        buf.push_at(point(5 * SEC), 5 * SEC);
        buf.persist().unwrap();

        // Une heure plus tard, le point doit toujours porter sa date d'origine.
        let relu = ExportBuffer::open_at(path, 3605 * SEC);
        let (_, body) = relu.batch().unwrap();
        assert_eq!(body, point(5 * SEC));
    }

    #[test]
    fn le_rechargement_jette_les_points_trop_vieux() {
        let path = tmp_path("vieux");
        let _ = std::fs::remove_file(&path);
        let mut buf =
            ExportBuffer::open_with_limits_at(path.clone(), 100, Duration::from_secs(10), 1 * SEC);
        buf.push_at(point(1 * SEC), 1 * SEC);
        buf.push_at(point(2 * SEC), 2 * SEC);
        buf.persist().unwrap();

        let relu =
            ExportBuffer::open_with_limits_at(path, 100, Duration::from_secs(10), 100 * SEC);
        assert!(relu.is_empty(), "des points d'il y a 100 s ne survivent pas à une borne de 10 s");
    }

    #[test]
    fn la_confirmation_est_persistee() {
        let path = tmp_path("confirm");
        let _ = std::fs::remove_file(&path);
        let mut buf = ExportBuffer::open_at(path.clone(), 1 * SEC);
        buf.push_at(point(1 * SEC), 1 * SEC);
        buf.push_at(point(2 * SEC), 2 * SEC);
        buf.confirm(1);
        buf.persist().unwrap();

        let relu = ExportBuffer::open_at(path, 3 * SEC);
        assert_eq!(relu.len(), 1);
        let (_, body) = relu.batch().unwrap();
        assert_eq!(body, point(2 * SEC));
    }

    #[test]
    fn un_tampon_persiste_n_est_plus_sale() {
        let path = tmp_path("sale");
        let _ = std::fs::remove_file(&path);
        let mut buf = ExportBuffer::open_at(path, 1 * SEC);
        assert!(!buf.is_dirty(), "un tampon neuf est déjà en phase avec son fichier");
        buf.push_at(point(1 * SEC), 1 * SEC);
        assert!(buf.is_dirty());
        buf.persist().unwrap();
        assert!(!buf.is_dirty());
    }

    #[test]
    fn un_fichier_illisible_laisse_le_tampon_vide_au_lieu_de_planter() {
        let path = tmp_path("illisible");
        std::fs::write(&path, "ceci n'est pas du ndjson\n{").unwrap();
        let buf = ExportBuffer::open_at(path, 1 * SEC);
        assert!(buf.is_empty());
    }

    #[test]
    fn abandonner_le_tampon_le_vide_et_le_marque_sale() {
        // Export désactivé par l'utilisateur : c'est un choix explicite, pas
        // une panne — et le disque doit oublier avec la mémoire.
        let path = tmp_path("abandon");
        let _ = std::fs::remove_file(&path);
        let mut buf = ExportBuffer::open_at(path.clone(), 1 * SEC);
        buf.push_at(point(1 * SEC), 1 * SEC);
        buf.persist().unwrap();

        buf.discard_all();
        assert!(buf.is_empty());
        assert!(buf.is_dirty());
        buf.persist().unwrap();

        assert!(ExportBuffer::open_at(path, 2 * SEC).is_empty());
    }

    #[test]
    fn un_tampon_vide_n_a_pas_de_lot() {
        let buf = ExportBuffer::new();
        assert!(buf.is_empty());
        assert!(buf.batch().is_none());
    }

    /// Destination de test : enregistre les corps reçus, refuse les
    /// `failures` premières écritures.
    struct FakeWriter {
        failures: std::sync::Mutex<usize>,
        bodies: std::sync::Mutex<Vec<String>>,
    }

    impl FakeWriter {
        fn refusing(failures: usize) -> Self {
            Self {
                failures: std::sync::Mutex::new(failures),
                bodies: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn bodies(&self) -> Vec<String> {
            self.bodies.lock().unwrap().clone()
        }
    }

    impl PointWriter for FakeWriter {
        async fn write_points(&self, body: String) -> Result<(), String> {
            self.bodies.lock().unwrap().push(body);
            let mut left = self.failures.lock().unwrap();
            if *left > 0 {
                *left -= 1;
                Err("influx injoignable".to_string())
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn une_ecriture_refusee_ne_perd_aucun_point() {
        let writer = FakeWriter::refusing(1);
        let mut buf = ExportBuffer::new();
        buf.push_at(point(1 * SEC), 1 * SEC);
        buf.push_at(point(2 * SEC), 2 * SEC);
        let mut backoff = Backoff::new();

        let out = flush_once(&writer, &mut buf, &mut backoff, std::time::Instant::now()).await;

        assert!(matches!(out, FlushOutcome::Failed(_)), "{out:?}");
        assert_eq!(buf.len(), 2, "le tampon garde tout tant que rien n'est confirmé");
    }

    #[tokio::test]
    async fn une_ecriture_confirmee_retire_le_lot() {
        let writer = FakeWriter::refusing(0);
        let mut buf = ExportBuffer::new();
        buf.push_at(point(1 * SEC), 1 * SEC);
        let mut backoff = Backoff::new();

        let out = flush_once(&writer, &mut buf, &mut backoff, std::time::Instant::now()).await;

        assert_eq!(out, FlushOutcome::Written(1));
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn le_rejeu_renvoie_les_lignes_datees_de_la_mesure() {
        let writer = FakeWriter::refusing(1);
        let mut buf = ExportBuffer::new();
        buf.push_at(point(1 * SEC), 1 * SEC);
        let mut backoff = Backoff::new();
        let t0 = std::time::Instant::now();

        flush_once(&writer, &mut buf, &mut backoff, t0).await;
        // Bien plus tard : le repli est écoulé, on rejoue.
        let out = flush_once(&writer, &mut buf, &mut backoff, t0 + Duration::from_secs(120)).await;

        assert_eq!(out, FlushOutcome::Written(1));
        assert_eq!(
            writer.bodies(),
            vec![point(1 * SEC), point(1 * SEC)],
            "le rejeu ne doit pas re-dater le point"
        );
    }

    #[tokio::test]
    async fn le_repli_epargne_la_destination_entre_deux_tentatives() {
        let writer = FakeWriter::refusing(5);
        let mut buf = ExportBuffer::new();
        buf.push_at(point(1 * SEC), 1 * SEC);
        let mut backoff = Backoff::new();
        let t0 = std::time::Instant::now();

        flush_once(&writer, &mut buf, &mut backoff, t0).await;
        let out = flush_once(&writer, &mut buf, &mut backoff, t0 + Duration::from_millis(500)).await;

        assert_eq!(out, FlushOutcome::Skipped);
        assert_eq!(writer.bodies().len(), 1, "la destination ne doit pas être re-sollicitée");
    }

    #[tokio::test]
    async fn un_tampon_vide_ne_sollicite_pas_la_destination() {
        let writer = FakeWriter::refusing(0);
        let mut buf = ExportBuffer::new();
        let mut backoff = Backoff::new();

        let out = flush_once(&writer, &mut buf, &mut backoff, std::time::Instant::now()).await;

        assert_eq!(out, FlushOutcome::Skipped);
        assert!(writer.bodies().is_empty());
    }

    #[tokio::test]
    async fn les_points_arrives_pendant_l_echec_partent_au_rejeu() {
        let writer = FakeWriter::refusing(1);
        let mut buf = ExportBuffer::new();
        buf.push_at(point(1 * SEC), 1 * SEC);
        let mut backoff = Backoff::new();
        let t0 = std::time::Instant::now();

        flush_once(&writer, &mut buf, &mut backoff, t0).await;
        buf.push_at(point(2 * SEC), 2 * SEC);
        let out = flush_once(&writer, &mut buf, &mut backoff, t0 + Duration::from_secs(2)).await;

        assert_eq!(out, FlushOutcome::Written(2));
        assert_eq!(
            writer.bodies()[1],
            format!("{}\n{}", point(1 * SEC), point(2 * SEC))
        );
    }

    #[test]
    fn aucun_repli_avant_le_premier_echec() {
        let b = Backoff::new();
        assert_eq!(b.delay(), Duration::ZERO);
        assert!(b.ready_at(std::time::Instant::now()));
    }

    #[test]
    fn le_repli_double_a_chaque_echec() {
        let mut b = Backoff::new();
        let t0 = std::time::Instant::now();
        assert_eq!(b.record_failure_at(t0), Duration::from_secs(1));
        assert_eq!(b.record_failure_at(t0), Duration::from_secs(2));
        assert_eq!(b.record_failure_at(t0), Duration::from_secs(4));
        assert_eq!(b.record_failure_at(t0), Duration::from_secs(8));
    }

    #[test]
    fn le_repli_plafonne_a_soixante_secondes() {
        let mut b = Backoff::new();
        let t0 = std::time::Instant::now();
        for _ in 0..20 {
            b.record_failure_at(t0);
        }
        assert_eq!(b.delay(), Duration::from_secs(60));
    }

    #[test]
    fn le_repli_bloque_les_tentatives_avant_l_echeance() {
        let mut b = Backoff::new();
        let t0 = std::time::Instant::now();
        b.record_failure_at(t0);
        assert!(!b.ready_at(t0 + Duration::from_millis(999)));
        assert!(b.ready_at(t0 + Duration::from_secs(1)));
    }

    #[test]
    fn un_succes_remet_le_repli_a_zero() {
        let mut b = Backoff::new();
        let t0 = std::time::Instant::now();
        b.record_failure_at(t0);
        b.record_failure_at(t0);
        b.reset();
        assert_eq!(b.delay(), Duration::ZERO);
        assert!(b.ready_at(t0));
        // Le repli qui suit repart de la base, pas de là où il s'était arrêté.
        assert_eq!(b.record_failure_at(t0), Duration::from_secs(1));
    }
}
