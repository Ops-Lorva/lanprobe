use std::net::Ipv4Addr;
use super::proc::sync_cmd;

/// Valide qu'une chaîne est une adresse IPv4 valide (ou vide pour les champs
/// optionnels). Empêche l'injection de commandes via les champs IP dans les
/// scripts netsh/nmcli. Le nom d'interface, lui, est validé séparément par
/// `validate_interface`.
fn validate_ipv4(val: &str, field: &str) -> Result<(), String> {
    if val.is_empty() { return Ok(()); }
    val.parse::<Ipv4Addr>()
        .map_err(|_| format!("'{field}' n'est pas une adresse IPv4 valide : {val}"))?;
    Ok(())
}

/// Valide un nom d'interface avant de l'interpoler dans un script exécuté en
/// élévation (le `.bat` netsh sous Windows). Allowlist stricte : lettres,
/// chiffres, espaces, tirets et parenthèses — ce qui couvre les noms Windows
/// réels ("Ethernet", "Wi-Fi 2", "Ethernet (2)") tout en rejetant les
/// métacaractères batch (guillemets, `&`, `|`, `%`, `^`, `<`, `>`) qui
/// permettraient une exécution de commande arbitraire en administrateur.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn validate_interface(interface: &str) -> Result<(), String> {
    if interface.is_empty() {
        return Err("le nom d'interface est vide".to_string());
    }
    let ok = interface
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '(' | ')'));
    if !ok {
        return Err(format!(
            "nom d'interface invalide (caractères non autorisés) : {interface}"
        ));
    }
    Ok(())
}

/// Lance une commande networksetup avec les bons droits.
/// - Si la règle sudoers est installée : sudo -n (sans password)
/// - Sinon : AppleScript (prompt password)
#[cfg(target_os = "macos")]
fn networksetup(args: &[&str]) -> Result<String, String> {
    use super::permissions::has_permissions;

    if has_permissions() {
        let mut full_args = vec!["-n", "/usr/sbin/networksetup"];
        full_args.extend_from_slice(args);
        let out = sync_cmd("sudo")
            .args(&full_args)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        // sudo -n a échoué (règle expirée ?) → fallback AppleScript
    }

    // AppleScript fallback
    let cmd_str = format!("networksetup {}", args.iter().map(|a| format!("'{}'", a.replace('\'', "'\\''"))).collect::<Vec<_>>().join(" "));
    let script = format!(r#"do shell script "{}" with administrator privileges"#, cmd_str.replace('"', "\\\""));
    let out = sync_cmd("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub struct NetworkConfig {
    pub interface: String,
    pub ip: String,
    pub subnet: String,
    // Gateway et DNS sont optionnels : pour des tests isolés (réseau sans
    // routeur, banc d'essai avec un seul switch, etc.) on peut vouloir
    // poser juste une IP/mask. Chaîne vide = "ne rien pousser".
    pub gateway: String,
    pub dns_primary: String,
    pub dns_secondary: Option<String>,
}

pub fn apply_static(config: &NetworkConfig) -> Result<(), String> {
    // Valide les adresses IPv4 avant de les insérer dans un script. Le nom
    // d'interface est validé par `validate_interface` dans l'implémentation
    // Windows, juste avant la construction du .bat exécuté en élévation.
    validate_ipv4(&config.ip, "ip")?;
    validate_ipv4(&config.subnet, "subnet")?;
    validate_ipv4(&config.gateway, "gateway")?;
    validate_ipv4(&config.dns_primary, "dns_primary")?;
    if let Some(ref d2) = config.dns_secondary {
        validate_ipv4(d2, "dns_secondary")?;
    }
    #[cfg(target_os = "windows")]
    return apply_static_windows(config);
    #[cfg(target_os = "macos")]
    return apply_static_macos(config);
    #[cfg(target_os = "linux")]
    return apply_static_linux(config);
    #[allow(unreachable_code)]
    Err("Unsupported platform".to_string())
}

pub fn apply_dhcp(interface: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    return apply_dhcp_windows(interface);
    #[cfg(target_os = "macos")]
    return apply_dhcp_macos(interface);
    #[cfg(target_os = "linux")]
    return apply_dhcp_linux(interface);
    #[allow(unreachable_code)]
    Err("Unsupported platform".to_string())
}

#[cfg(target_os = "windows")]
fn apply_static_windows(config: &NetworkConfig) -> Result<(), String> {
    // Le nom d'interface est interpolé tel quel dans le .bat lancé en admin :
    // on le valide AVANT de construire le script.
    validate_interface(&config.interface)?;
    let mut script = String::from("@echo off\r\n");
    // Pose l'IP/mask. Si la gateway est renseignée, on l'ajoute avec une
    // métrique de 1 ; sinon on laisse netsh n'écrire que l'adresse.
    if !config.gateway.is_empty() {
        script.push_str(&format!(
            "netsh interface ip set address name=\"{}\" static {} {} {} 1\r\nif errorlevel 1 exit /b 1\r\n",
            config.interface, config.ip, config.subnet, config.gateway
        ));
    } else {
        script.push_str(&format!(
            "netsh interface ip set address name=\"{}\" static {} {}\r\nif errorlevel 1 exit /b 1\r\n",
            config.interface, config.ip, config.subnet
        ));
    }
    // DNS : si primaire vide, on force "dhcp" (purge toute config DNS
    // statique héritée d'un profil précédent).
    if !config.dns_primary.is_empty() {
        script.push_str(&format!(
            "netsh interface ip set dns name=\"{}\" static {} primary\r\nif errorlevel 1 exit /b 1\r\n",
            config.interface, config.dns_primary
        ));
        if let Some(dns2) = config.dns_secondary.as_deref() {
            if !dns2.is_empty() {
                script.push_str(&format!(
                    "netsh interface ip add dns name=\"{}\" {} index=2\r\nif errorlevel 1 exit /b 1\r\n",
                    config.interface, dns2
                ));
            }
        }
    } else {
        script.push_str(&format!(
            "netsh interface ip set dns name=\"{}\" dhcp\r\nif errorlevel 1 exit /b 1\r\n",
            config.interface
        ));
    }
    run_elevated_bat(&script)
}

#[cfg(target_os = "windows")]
fn apply_dhcp_windows(interface: &str) -> Result<(), String> {
    // Idem : valide le nom d'interface avant de l'injecter dans le .bat.
    validate_interface(interface)?;
    let script = format!(
        "@echo off\r\n\
         netsh interface ip set address name=\"{0}\" dhcp\r\nif errorlevel 1 exit /b 1\r\n\
         netsh interface ip set dns name=\"{0}\" dhcp\r\nif errorlevel 1 exit /b 1\r\n",
        interface
    );
    run_elevated_bat(&script)
}

/// Writes `script` to a temp .bat file and runs it elevated via
/// `powershell Start-Process -Verb RunAs` so the user sees a single UAC prompt
/// for the whole apply operation.
#[cfg(target_os = "windows")]
fn run_elevated_bat(script: &str) -> Result<(), String> {
    let temp = std::env::temp_dir().join(format!("lanprobe-apply-{}.bat", std::process::id()));
    std::fs::write(&temp, script).map_err(|e| e.to_string())?;
    let ps_cmd = format!(
        "$p = Start-Process -FilePath '{}' -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode",
        temp.to_string_lossy().replace('\'', "''")
    );
    let out = sync_cmd("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
        .output()
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&temp);
    let out = out?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if stderr.contains("canceled") || stderr.contains("annul") {
            return Err("Opération annulée par l'utilisateur".to_string());
        }
        return Err(if stderr.is_empty() {
            format!("Échec de l'application (code {})", out.status.code().unwrap_or(-1))
        } else {
            stderr
        });
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn apply_static_macos(config: &NetworkConfig) -> Result<(), String> {
    // networksetup -setmanual exige ip/mask/gateway. Si la gateway est
    // vide, on passe l'IP elle-même comme placeholder (équivaut à "pas de
    // route par défaut sortante") — c'est le workaround standard macOS.
    let gw = if config.gateway.is_empty() { config.ip.as_str() } else { config.gateway.as_str() };
    networksetup(&["-setmanual", &config.interface, &config.ip, &config.subnet, gw])?;
    if config.dns_primary.is_empty() {
        networksetup(&["-setdnsservers", &config.interface, "empty"])?;
    } else {
        let mut dns_args = vec!["-setdnsservers", &config.interface, &config.dns_primary];
        let dns2 = config.dns_secondary.as_deref().unwrap_or("");
        if !dns2.is_empty() { dns_args.push(dns2); }
        networksetup(&dns_args)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn apply_dhcp_macos(interface: &str) -> Result<(), String> {
    networksetup(&["-setdhcp", interface])?;
    networksetup(&["-setdnsservers", interface, "empty"])?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn resolve_nm_connection(device: &str) -> Result<String, String> {
    // nmcli `con mod` attend un nom de connexion, pas un device. On résout
    // d'abord la connexion active attachée au device (`GENERAL.CONNECTION`).
    // Si aucune n'est active, on cherche une connexion persistée dont
    // `connection.interface-name` matche le device. En dernier recours, on
    // en crée une nouvelle pour ce device.
    let out = sync_cmd("nmcli")
        .args(["-t", "-g", "GENERAL.CONNECTION", "device", "show", device])
        .output().map_err(|e| e.to_string())?;
    if out.status.success() {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !name.is_empty() && name != "--" {
            return Ok(name);
        }
    }
    // Pas de connexion active : chercher dans la liste des connexions persistées.
    let out = sync_cmd("nmcli")
        .args(["-t", "-f", "NAME,DEVICE", "connection", "show"])
        .output().map_err(|e| e.to_string())?;
    if out.status.success() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some((name, dev)) = line.split_once(':') {
                if dev == device && !name.is_empty() {
                    return Ok(name.to_string());
                }
            }
        }
    }
    // Dernier recours : créer une connexion ethernet pour ce device.
    let name = format!("lanprobe-{}", device);
    let out = sync_cmd("nmcli")
        .args(["connection", "add", "type", "ethernet",
               "con-name", &name, "ifname", device])
        .output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "Impossible de résoudre une connexion NetworkManager pour {} : {}",
            device,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(name)
}

#[cfg(target_os = "linux")]
fn apply_static_linux(config: &NetworkConfig) -> Result<(), String> {
    let conn = resolve_nm_connection(&config.interface)?;
    let prefix = mask_to_cidr(&config.subnet);
    let addr = format!("{}/{}", config.ip, prefix);
    // nmcli accepte une chaîne vide pour effacer un champ : on passe "" quand
    // l'utilisateur n'a pas renseigné de gateway/DNS (bancs de test isolés).
    let dns = match config.dns_secondary.as_deref() {
        Some(d2) if !d2.is_empty() && !config.dns_primary.is_empty() => format!("{} {}", config.dns_primary, d2),
        _ => config.dns_primary.clone(),
    };
    let out = sync_cmd("nmcli")
        .args(["con", "mod", &conn,
            "ipv4.addresses", &addr,
            "ipv4.gateway", &config.gateway,
            "ipv4.dns", &dns,
            "ipv4.method", "manual"])
        .output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let out = sync_cmd("nmcli").args(["con", "up", &conn]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_dhcp_linux(interface: &str) -> Result<(), String> {
    let conn = resolve_nm_connection(interface)?;
    let out = sync_cmd("nmcli")
        .args(["con", "mod", &conn, "ipv4.method", "auto",
               "ipv4.addresses", "", "ipv4.gateway", "", "ipv4.dns", ""])
        .output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let out = sync_cmd("nmcli").args(["con", "up", &conn]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    Ok(())
}

fn mask_to_cidr(mask: &str) -> u32 {
    mask.split('.').fold(0u32, |acc, octet| acc + octet.parse::<u32>().unwrap_or(0).count_ones())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_to_cidr_24() {
        assert_eq!(mask_to_cidr("255.255.255.0"), 24);
    }

    #[test]
    fn test_mask_to_cidr_16() {
        assert_eq!(mask_to_cidr("255.255.0.0"), 16);
    }

    #[test]
    fn validate_interface_accepts_real_windows_names() {
        for name in ["Ethernet", "Wi-Fi", "Wi-Fi 2", "Ethernet (2)", "Local Area Connection"] {
            assert!(validate_interface(name).is_ok(), "devrait accepter : {name}");
        }
    }

    #[test]
    fn validate_interface_rejects_batch_metacharacters() {
        for name in [
            "Ethernet\" & calc.exe",
            "Wi-Fi\"",
            "eth&whoami",
            "a|b",
            "%PATH%",
            "a^b",
            "a<b",
            "a>b",
            "",
        ] {
            assert!(validate_interface(name).is_err(), "devrait rejeter : {name:?}");
        }
    }
}
