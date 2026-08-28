//! Snapshot unique de la config frontend (settings, profils réseau,
//! profils portscan) détenu côté backend et synchronisé via le bus
//! d'events `config:update`. C'est ce qui permet à un client web et au
//! desktop Tauri de rester en phase : toute modif sur l'un est push sur
//! l'autre à travers `broadcast::Sender<BroadcastEvent>`.
//!
//! Migration : la version précédente stockait la config dans
//! `<data_dir>/lanprobe/config.json` via tauri-plugin-store. Au premier
//! lancement, si on trouve l'ancien fichier mais pas le nouveau, on le
//! recopie pour ne pas perdre les profils/settings de l'utilisateur.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::Value;

use crate::secrets::{self, SecretKey};

/// Chemins des seules valeurs chiffrées au repos. On garde volontairement la
/// liste courte : l'URL, l'org et le bucket restent lisibles pour pouvoir
/// diagnostiquer une instance sans clé. Ce sont bien des secrets applicatifs
/// — les mots de passe utilisateur vivent hashés dans `users.json`.
const SECRET_PATHS: &[&[&str]] = &[
    &["influxdb", "v1", "password"],
    &["influxdb", "v2", "token"],
];

pub struct ConfigStore {
    path: PathBuf,
    data: Mutex<Value>,
    /// `Err` quand aucune clé utilisable n'a pu être obtenue : dans ce cas on
    /// refuse d'écrire un secret plutôt que de le poser en clair.
    key: Result<SecretKey, String>,
}

impl ConfigStore {
    pub fn load(path: PathBuf) -> Self {
        if !path.exists() {
            if let Some(old_dir) = dirs_next::data_dir() {
                let old = old_dir.join("lanprobe").join("config.json");
                if old.exists() {
                    if let Ok(raw) = std::fs::read_to_string(&old) {
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(&path, &raw);
                    }
                }
            }
        }
        let mut data: Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({}));

        let config_dir = path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        let key = secrets::load_or_create_key(&config_dir);
        match &key {
            Ok(k) => unseal_secrets(&mut data, k),
            Err(e) => tracing::error!(
                "clé de chiffrement indisponible ({e}) — les secrets déjà scellés \
                 resteront illisibles et aucun nouveau secret ne sera écrit"
            ),
        }

        Self {
            path,
            data: Mutex::new(data),
            key,
        }
    }

    /// Répertoire qui porte la config — et, à côté, les états que le serveur
    /// doit retrouver après un redémarrage (clé de scellement, tampon
    /// d'export…).
    pub fn dir(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }

    pub fn get(&self) -> Value {
        self.data
            .lock()
            .map(|g| g.clone())
            .unwrap_or(Value::Null)
    }

    pub fn put(&self, new_data: Value) -> Result<(), String> {
        // On scelle une copie : la version en mémoire (et donc celle rendue au
        // frontend) reste en clair, sinon l'export InfluxDB et le formulaire
        // des Settings cesseraient de fonctionner.
        let mut on_disk = new_data.clone();
        let sealed_count = match &self.key {
            Ok(k) => seal_secrets(&mut on_disk, k)?,
            Err(e) => {
                if collect_secrets(&new_data) > 0 {
                    return Err(format!(
                        "impossible de chiffrer les secrets de la configuration : {e}"
                    ));
                }
                0
            }
        };
        let _ = sealed_count;

        let json = serde_json::to_string_pretty(&on_disk).map_err(|e| e.to_string())?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.path, json).map_err(|e| e.to_string())?;
        // Même traitement que `users.json` : le fichier porte des secrets,
        // il ne doit pas être lisible par le reste du système.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        if let Ok(mut g) = self.data.lock() {
            *g = new_data;
        }
        Ok(())
    }
}

/// Suit un chemin dans le JSON et rend la feuille si c'est une chaîne non vide.
fn leaf_mut<'a>(value: &'a mut Value, path: &[&str]) -> Option<&'a mut Value> {
    let mut cur = value;
    for key in path {
        cur = cur.as_object_mut()?.get_mut(*key)?;
    }
    match cur {
        Value::String(s) if !s.is_empty() => Some(cur),
        _ => None,
    }
}

/// Scelle en place chaque secret présent. Rend le nombre de valeurs scellées.
fn seal_secrets(value: &mut Value, key: &SecretKey) -> Result<usize, String> {
    let mut sealed = 0;
    for path in SECRET_PATHS {
        let Some(leaf) = leaf_mut(value, path) else { continue };
        let plain = leaf.as_str().unwrap_or_default().to_string();
        if secrets::is_sealed(&plain) {
            continue; // déjà scellé (config relue puis réécrite sans changement)
        }
        *leaf = Value::String(secrets::seal(key, &plain)?);
        sealed += 1;
    }
    Ok(sealed)
}

/// Ouvre en place chaque secret scellé. Une valeur en clair est laissée telle
/// quelle — c'est le chemin de migration depuis les versions antérieures.
/// Une valeur scellée qu'on n'arrive pas à ouvrir est laissée intacte plutôt
/// qu'effacée : on ne détruit pas la donnée d'un utilisateur sur un doute.
fn unseal_secrets(value: &mut Value, key: &SecretKey) {
    for path in SECRET_PATHS {
        let Some(leaf) = leaf_mut(value, path) else { continue };
        let stored = leaf.as_str().unwrap_or_default().to_string();
        if !secrets::is_sealed(&stored) {
            continue;
        }
        match secrets::open(key, &stored) {
            Ok(plain) => *leaf = Value::String(plain),
            Err(e) => tracing::error!("secret de configuration illisible ({e})"),
        }
    }
}

/// Compte les secrets non vides présents dans la config.
fn collect_secrets(value: &Value) -> usize {
    let mut probe = value.clone();
    SECRET_PATHS
        .iter()
        .filter(|path| leaf_mut(&mut probe, path).is_some())
        .count()
}

pub fn default_config_path(config_dir: &Path) -> PathBuf {
    config_dir.join("app_config.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lanprobe-config-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn sample() -> Value {
        json!({
            "influxdb": {
                "enabled": true,
                "version": "v2",
                "url": "http://influx.lan:8086",
                "v1": { "database": "lanprobe", "username": "probe", "password": "MOT-DE-PASSE-V1" },
                "v2": { "org": "lorva", "bucket": "lanprobe", "token": "JETON-INFLUX-V2" }
            },
            "settings": { "theme": "dark" }
        })
    }

    #[test]
    fn le_repertoire_est_celui_du_fichier_de_config() {
        let _guard = crate::secrets::ENV_KEY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tmp_dir("dir");
        let store = ConfigStore::load(default_config_path(&dir));
        assert_eq!(store.dir(), dir);
    }

    #[test]
    fn put_seals_influx_secrets_on_disk() {
        let _guard = crate::secrets::ENV_KEY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tmp_dir("seal");
        let path = default_config_path(&dir);
        let _ = std::fs::remove_file(&path);

        let store = ConfigStore::load(path.clone());
        store.put(sample()).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("JETON-INFLUX-V2"),
            "le jeton v2 ne doit pas être en clair sur disque :\n{raw}"
        );
        assert!(
            !raw.contains("MOT-DE-PASSE-V1"),
            "le mot de passe v1 ne doit pas être en clair sur disque :\n{raw}"
        );

        let on_disk: Value = serde_json::from_str(&raw).unwrap();
        assert!(crate::secrets::is_sealed(
            on_disk["influxdb"]["v2"]["token"].as_str().unwrap()
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_secret_fields_stay_readable_on_disk() {
        let _guard = crate::secrets::ENV_KEY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tmp_dir("readable");
        let path = default_config_path(&dir);
        let _ = std::fs::remove_file(&path);

        ConfigStore::load(path.clone()).put(sample()).unwrap();

        // On ne chiffre que ce qui est secret : l'URL, l'org, le bucket et
        // les settings restent lisibles pour diagnostiquer une instance.
        let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk["influxdb"]["url"], "http://influx.lan:8086");
        assert_eq!(on_disk["influxdb"]["v2"]["bucket"], "lanprobe");
        assert_eq!(on_disk["influxdb"]["v1"]["username"], "probe");
        assert_eq!(on_disk["settings"]["theme"], "dark");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_returns_the_plaintext_secrets() {
        let _guard = crate::secrets::ENV_KEY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tmp_dir("reload");
        let path = default_config_path(&dir);
        let _ = std::fs::remove_file(&path);

        ConfigStore::load(path.clone()).put(sample()).unwrap();

        // Redémarrage du serveur : influxdb.rs doit retrouver un jeton
        // utilisable, sinon l'export casse silencieusement.
        let reloaded = ConfigStore::load(path.clone());
        let data = reloaded.get();
        assert_eq!(data["influxdb"]["v2"]["token"], "JETON-INFLUX-V2");
        assert_eq!(data["influxdb"]["v1"]["password"], "MOT-DE-PASSE-V1");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_plaintext_config_is_read_then_sealed_on_next_write() {
        let _guard = crate::secrets::ENV_KEY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tmp_dir("legacy");
        let path = default_config_path(&dir);
        // Fichier écrit par une version antérieure : tout est en clair.
        std::fs::write(&path, serde_json::to_string_pretty(&sample()).unwrap()).unwrap();

        let store = ConfigStore::load(path.clone());
        assert_eq!(
            store.get()["influxdb"]["v2"]["token"], "JETON-INFLUX-V2",
            "une config héritée doit rester lisible"
        );

        store.put(store.get()).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("JETON-INFLUX-V2"), "la réécriture doit sceller :\n{raw}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn config_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = crate::secrets::ENV_KEY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tmp_dir("perms");
        let path = default_config_path(&dir);
        let _ = std::fs::remove_file(&path);

        ConfigStore::load(path.clone()).put(sample()).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "app_config.json doit être 0600");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn put_without_secret_works_even_without_a_usable_key() {
        let _guard = crate::secrets::ENV_KEY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tmp_dir("nokey");
        let path = default_config_path(&dir);
        let _ = std::fs::remove_file(&path);

        // SAFETY : verrou pris, la variable est restaurée avant de rendre.
        unsafe { std::env::set_var(crate::secrets::KEY_ENV, "pas-du-base64-32-octets") };
        let store = ConfigStore::load(path.clone());
        let no_secret = store.put(json!({ "settings": { "theme": "light" } }));
        let with_secret = store.put(sample());
        unsafe { std::env::remove_var(crate::secrets::KEY_ENV) };

        assert!(no_secret.is_ok(), "une config sans secret n'a pas besoin de clé");
        // Refus explicite plutôt que dégradation silencieuse en clair.
        assert!(
            with_secret.is_err(),
            "sans clé utilisable, un secret ne doit jamais être écrit en clair"
        );
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("JETON-INFLUX-V2"), "rien ne doit avoir fuité :\n{raw}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
