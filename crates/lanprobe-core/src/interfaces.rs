use serde::{Deserialize, Serialize};
use super::proc::sync_cmd;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct InterfaceDetails {
    pub name: String,
    pub ip: Option<String>,
    pub subnet: Option<String>,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
    pub dhcp_enabled: bool,
    pub is_up: bool,
    /// Nom BSD réel de l'interface (en0, en1…). Populé sur macOS, où `name`
    /// est le service networksetup ("Wi-Fi", "Ethernet") — les CLI comme
    /// `speedtest -I` attendent l'ifname BSD et pas le service.
    #[serde(default)]
    pub bsd_name: Option<String>,
}

/// Mappe un service networksetup ("Wi-Fi") vers l'ifname BSD ("en0") en
/// parsant `networksetup -listallhardwareports`.
#[cfg(target_os = "macos")]
pub fn bsd_name_for_service(service: &str) -> Option<String> {
    let text = sync_cmd("networksetup").arg("-listallhardwareports").output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let mut current_port: Option<String> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("Hardware Port:") {
            current_port = Some(p.trim().to_string());
        } else if let Some(dev) = line.strip_prefix("Device:") {
            if current_port.as_deref() == Some(service) {
                return Some(dev.trim().to_string());
            }
        }
    }
    None
}

pub fn list_interfaces() -> Vec<String> {
    #[cfg(target_os = "windows")]
    { list_interfaces_windows() }
    #[cfg(target_os = "macos")]
    { list_interfaces_macos() }
    #[cfg(target_os = "linux")]
    { list_interfaces_linux() }
}

pub fn get_interface_details(name: &str) -> InterfaceDetails {
    #[cfg(target_os = "windows")]
    { get_details_windows(name) }
    #[cfg(target_os = "macos")]
    { get_details_macos(name) }
    #[cfg(target_os = "linux")]
    { get_details_linux(name) }
}

#[cfg(target_os = "windows")]
fn list_interfaces_windows() -> Vec<String> {
    // PowerShell Get-NetAdapter — locale-indépendant. On force UTF-8 pour
    // éviter que PowerShell retourne du UTF-16 / code-page système (problème
    // courant sur Windows FR qui casse les noms d'interfaces accentués).
    // HardwareInterface=$true exclut Wintun, vEthernet, Hyper-V, loopback.
    let ps = r#"[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Get-NetAdapter | Where-Object { $_.HardwareInterface -eq $true } | Select-Object -ExpandProperty Name"#;
    let out = sync_cmd("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps])
        .output();
    if let Ok(o) = out {
        let text = String::from_utf8_lossy(&o.stdout).into_owned();
        let list: Vec<String> = text.lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        if !list.is_empty() {
            return list;
        }
    }
    // Fallback minimal si PowerShell est indisponible (rare) — on accepte les
    // interfaces IPv4 listées par `netsh interface ipv4 show interfaces` et on
    // rejette explicitement les noms de tunnel/virtuels connus.
    let text = sync_cmd("netsh")
        .args(["interface", "ipv4", "show", "interfaces"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    text.lines()
        .skip(3)
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let _idx = it.next()?;
            let _met = it.next()?;
            let _mtu = it.next()?;
            let _state = it.next()?;
            let name = it.collect::<Vec<_>>().join(" ");
            if name.is_empty() { return None; }
            let low = name.to_lowercase();
            if low.contains("wintun") || low.contains("loopback") || low.contains("vethernet") || low.contains("hyper-v") || low.contains("isatap") || low.contains("teredo") { return None; }
            Some(name)
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn list_interfaces_macos() -> Vec<String> {
    let text = sync_cmd("networksetup")
        .arg("-listallnetworkservices")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    text.lines()
        .skip(1)
        .filter(|l| !l.starts_with('*') && !l.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(target_os = "linux")]
fn list_interfaces_linux() -> Vec<String> {
    let text = sync_cmd("ip").args(["link", "show"]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    text.lines()
        .filter(|l| !l.starts_with(' ') && l.contains(": "))
        .filter_map(|l| {
            let name = l.split(':').nth(1)?.trim().split('@').next()?.to_string();
            if name != "lo" { Some(name) } else { None }
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn get_details_windows(name: &str) -> InterfaceDetails {
    // On utilise PowerShell avec des APIs .NET locale-indépendantes plutôt que
    // `netsh` dont la sortie est localisée (FR: "Adresse IP :", EN: "IP Address:").
    // Format de sortie : "IP|PrefixLen|Gateway|DNS1,DNS2|DHCP"
    // Les valeurs absentes sont vides (ex: "||" si pas de gateway).
    let ps = format!(
        r#"[Console]::OutputEncoding = [System.Text.Encoding]::UTF8;
$a = Get-NetIPAddress -InterfaceAlias '{name}' -AddressFamily IPv4 -ErrorAction SilentlyContinue | Select-Object -First 1;
$g = (Get-NetRoute -InterfaceAlias '{name}' -DestinationPrefix '0.0.0.0/0' -ErrorAction SilentlyContinue | Select-Object -First 1).NextHop;
$dns = (Get-DnsClientServerAddress -InterfaceAlias '{name}' -AddressFamily IPv4 -ErrorAction SilentlyContinue).ServerAddresses -join ',';
$dhcp = (Get-NetIPInterface -InterfaceAlias '{name}' -AddressFamily IPv4 -ErrorAction SilentlyContinue).Dhcp;
Write-Output "$($a.IPAddress)|$($a.PrefixLength)|$g|$dns|$dhcp""#,
        name = name.replace('\'', "''")
    );
    let mut d = InterfaceDetails { name: name.to_string(), is_up: true, ..Default::default() };
    let out = sync_cmd("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .output();
    if let Ok(o) = out {
        let text = String::from_utf8_lossy(&o.stdout).into_owned();
        // Cherche la première ligne non-vide qui ressemble à notre format
        if let Some(line) = text.lines().find(|l| l.contains('|')) {
            let parts: Vec<&str> = line.trim().splitn(5, '|').collect();
            // IP
            if let Some(ip) = parts.first().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                d.ip = Some(ip.to_string());
            }
            // PrefixLength → mask dotted
            if let Some(prefix) = parts.get(1).and_then(|s| s.trim().parse::<u32>().ok()) {
                let mask = if prefix == 0 { 0u32 } else { !0u32 << (32 - prefix) };
                d.subnet = Some(format!(
                    "{}.{}.{}.{}",
                    (mask >> 24) & 0xFF, (mask >> 16) & 0xFF,
                    (mask >> 8) & 0xFF, mask & 0xFF
                ));
            }
            // Gateway
            if let Some(gw) = parts.get(2).map(|s| s.trim()).filter(|s| !s.is_empty() && *s != "0.0.0.0") {
                d.gateway = Some(gw.to_string());
            }
            // DNS
            if let Some(dns_str) = parts.get(3).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                d.dns = dns_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            }
            // DHCP
            if let Some(dhcp) = parts.get(4).map(|s| s.trim().to_lowercase()) {
                d.dhcp_enabled = dhcp == "enabled";
            }
        }
    }
    d
}

#[cfg(target_os = "macos")]
fn get_details_macos(name: &str) -> InterfaceDetails {
    let text = sync_cmd("networksetup").args(["-getinfo", name]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let mut d = InterfaceDetails { name: name.to_string(), ip: None, subnet: None, gateway: None, dns: vec![], dhcp_enabled: false, is_up: true, bsd_name: bsd_name_for_service(name) };
    for line in text.lines() {
        if line.starts_with("IP address:") {
            d.ip = line.splitn(2, ':').nth(1).map(|s| s.trim().to_string());
        } else if line.starts_with("Subnet mask:") {
            d.subnet = line.splitn(2, ':').nth(1).map(|s| s.trim().to_string());
        } else if line.starts_with("Router:") {
            d.gateway = line.splitn(2, ':').nth(1).map(|s| s.trim().to_string());
        } else if line.contains("DHCP Configuration") {
            d.dhcp_enabled = true;
        }
    }
    let dns_text = sync_cmd("networksetup").args(["-getdnsservers", name]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    for line in dns_text.lines() {
        let l = line.trim();
        if !l.is_empty() && !l.starts_with("There") { d.dns.push(l.to_string()); }
    }
    // Fallback: DNS from DHCP via /etc/resolv.conf
    if d.dns.is_empty() {
        if let Ok(resolv) = std::fs::read_to_string("/etc/resolv.conf") {
            for line in resolv.lines() {
                if line.starts_with("nameserver") {
                    if let Some(dns) = line.split_whitespace().nth(1) {
                        d.dns.push(dns.to_string());
                    }
                }
            }
        }
    }
    d
}

/// Passerelle par défaut **portée par cette interface**, ou `None`.
///
/// 🔴 `ip route show default` décrit la route par défaut de la MACHINE, pas
/// celle de l'interface interrogée. La recopier sur toutes attribuait
/// `10.0.30.1` aux cinq `veth*` d'un hôte Docker — une valeur plausible sur
/// quelque chose qui ne la porte pas.
///
/// ⚠️ C'est l'endroit où ça compte le plus : cette liste est le seul repère de
/// qui choisit son interface sans écran. Une passerelle en face d'un `veth`
/// invite à le choisir, et une sonde posée sur le mauvais lien mesure faux
/// sans jamais le dire.
///
/// On lit par MOT-CLÉ (`via`, `dev`) et jamais par position : le noyau ajoute
/// `proto`, `metric`, `onlink`… selon les cas, et une lecture positionnelle
/// casserait au premier hôte configuré autrement. Une ligne sans `via` — une
/// route par défaut en `scope link` — ne porte aucune passerelle : `None`, et
/// surtout pas une valeur de remplacement qui se lirait comme une donnée.
#[cfg(any(target_os = "linux", test))]
fn gateway_for_device(route_output: &str, name: &str) -> Option<String> {
    fn after<'a>(fields: &[&'a str], key: &str) -> Option<&'a str> {
        fields.iter().position(|f| *f == key).and_then(|i| fields.get(i + 1)).copied()
    }
    route_output.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.first() != Some(&"default") || after(&fields, "dev") != Some(name) {
            return None;
        }
        after(&fields, "via").map(String::from)
    })
}

#[cfg(target_os = "linux")]
fn get_details_linux(name: &str) -> InterfaceDetails {
    let text = sync_cmd("ip").args(["addr", "show", name]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let text = text.as_str();
    let mut d = InterfaceDetails { name: name.to_string(), is_up: true, ..Default::default() };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("inet ") && !line.starts_with("inet6") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(cidr) = parts.get(1) {
                let mut split = cidr.splitn(2, '/');
                d.ip = split.next().map(String::from);
                if let Some(prefix) = split.next() {
                    d.subnet = prefix.parse::<u32>().ok().map(cidr_to_mask);
                }
            }
        }
    }
    let gw_text = sync_cmd("ip").args(["route", "show", "default"]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    d.gateway = gateway_for_device(&gw_text, name);
    if let Ok(resolv) = std::fs::read_to_string("/etc/resolv.conf") {
        for line in resolv.lines() {
            if line.starts_with("nameserver") {
                if let Some(dns) = line.split_whitespace().nth(1) {
                    d.dns.push(dns.to_string());
                }
            }
        }
    }
    // Récupère la méthode IPv4 via nmcli sur la connexion active attachée
    // au device. "auto" = DHCP, "manual" = statique. L'ancienne heuristique
    // `systemctl is-active NetworkManager` renvoyait toujours true et faisait
    // croire qu'un profil statique appliqué était en DHCP.
    let conn = sync_cmd("nmcli")
        .args(["-t", "-g", "GENERAL.CONNECTION", "device", "show", name])
        .output()
        .ok()
        .and_then(|o| if o.status.success() {
            Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
        } else { None })
        .filter(|s| !s.is_empty() && s != "--");
    if let Some(conn) = conn {
        let method = sync_cmd("nmcli")
            .args(["-t", "-g", "ipv4.method", "connection", "show", &conn])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        d.dhcp_enabled = method == "auto";
    }
    d
}

/// Adresse locale de l'interface au format CIDR (« 10.6.8.42/24 »).
///
/// C'est la forme que la sonde remonte au hub : l'adresse seule ne dit pas
/// quel réseau elle occupe, et deux sites en 192.168.1.x ne se distinguent
/// que par leur préfixe.
pub fn local_cidr(details: &InterfaceDetails) -> Option<String> {
    let ip = details.ip.as_deref()?.trim();
    if ip.is_empty() {
        return None;
    }
    match details.subnet.as_deref().and_then(mask_to_prefix) {
        Some(prefix) => Some(format!("{ip}/{prefix}")),
        // Sans masque exploitable on rend l'adresse nue : inventer un /32
        // ferait croire à un réseau point à point.
        None => Some(ip.to_string()),
    }
}

/// Longueur de préfixe d'un masque pointé. `None` si le masque n'en est pas
/// un — les bits à 1 doivent être contigus et en tête.
pub fn mask_to_prefix(mask: &str) -> Option<u32> {
    let mut value: u32 = 0;
    let mut octets = 0;
    for part in mask.trim().split('.') {
        let octet: u8 = part.parse().ok()?;
        value = (value << 8) | octet as u32;
        octets += 1;
    }
    if octets != 4 {
        return None;
    }
    let ones = value.leading_ones();
    // Un masque valide n'a plus aucun bit à 1 après sa tête contiguë.
    let expected = if ones == 0 { 0 } else { !0u32 << (32 - ones) };
    if value != expected {
        return None;
    }
    Some(ones)
}

fn cidr_to_mask(prefix: u32) -> String {
    if prefix == 0 { return "0.0.0.0".to_string(); }
    if prefix > 32 { return "255.255.255.255".to_string(); }
    let mask = !0u32 << (32 - prefix);
    format!("{}.{}.{}.{}", (mask >> 24) & 0xFF, (mask >> 16) & 0xFF, (mask >> 8) & 0xFF, mask & 0xFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn the_default_gateway_belongs_only_to_the_interface_that_carries_it() {
        // 🔴 `ip route show default` décrit LA route par défaut de la machine,
        // pas celle de l'interface qu'on interroge. Elle était recopiée sur
        // toutes : les cinq `veth*` de cette machine s'affichaient
        // « adresse — / passerelle 10.0.30.1 ».
        //
        // ⚠️ Une valeur plausible attribuée à quelque chose qui ne la porte
        // pas. Et à l'endroit où ça compte le plus : cette liste est le SEUL
        // repère de qui choisit son interface sans écran. Une passerelle en
        // face d'un `veth` invite à le choisir, et une sonde posée sur le
        // mauvais lien mesure faux sans jamais le dire.
        //
        // Sortie réelle de cette machine — noter l'absence de `proto`/`metric`
        // et la présence d'`onlink` : on lit par mot-clé, jamais par position.
        let reel = "default via 10.0.30.1 dev ens18 onlink \n";
        assert_eq!(gateway_for_device(reel, "ens18").as_deref(), Some("10.0.30.1"));
        assert_eq!(gateway_for_device(reel, "veth912ca4b"), None);
        assert_eq!(gateway_for_device(reel, "docker0"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn each_link_of_a_multi_wan_keeps_its_own_gateway() {
        // Deux sorties : chacune rend la sienne, aucune ne rend celle de
        // l'autre. Prendre la première ligne venue attribuait la passerelle du
        // lien principal au lien de secours.
        let deux = "default via 10.0.30.1 dev ens18 proto static metric 100\n                    default via 192.168.8.1 dev wwan0 proto dhcp metric 700\n";
        assert_eq!(gateway_for_device(deux, "ens18").as_deref(), Some("10.0.30.1"));
        assert_eq!(gateway_for_device(deux, "wwan0").as_deref(), Some("192.168.8.1"));
        assert_eq!(gateway_for_device(deux, "eth9"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn no_default_route_means_no_gateway_not_a_placeholder() {
        // ⚠️ Rien, et surtout pas une valeur « neutre » : une absence doit se
        // lire comme une absence, pas comme une donnée.
        assert_eq!(gateway_for_device("", "ens18"), None);
        assert_eq!(gateway_for_device("default dev tun0 scope link\n", "tun0"), None);
        assert_eq!(gateway_for_device("10.0.0.0/8 via 10.0.30.1 dev ens18\n", "ens18"), None);
    }

    #[test]
    fn test_list_interfaces_returns_vec() {
        let interfaces = list_interfaces();
        assert!(interfaces.len() >= 0);
    }

    #[test]
    fn test_cidr_to_mask_24() {
        assert_eq!(cidr_to_mask(24), "255.255.255.0");
    }

    #[test]
    fn test_cidr_to_mask_16() {
        assert_eq!(cidr_to_mask(16), "255.255.0.0");
    }

    #[test]
    fn local_cidr_combines_the_address_and_its_mask() {
        let d = InterfaceDetails {
            ip: Some("10.6.8.42".into()),
            subnet: Some("255.255.255.0".into()),
            ..Default::default()
        };
        assert_eq!(local_cidr(&d).as_deref(), Some("10.6.8.42/24"));
    }

    #[test]
    fn local_cidr_without_a_usable_mask_gives_the_bare_address() {
        // Mieux vaut une adresse sans préfixe qu'un /32 inventé : le
        // masque manquant est une information, pas un défaut à combler.
        let d = InterfaceDetails {
            ip: Some("10.6.8.42".into()),
            subnet: Some("pas un masque".into()),
            ..Default::default()
        };
        assert_eq!(local_cidr(&d).as_deref(), Some("10.6.8.42"));
    }

    #[test]
    fn local_cidr_of_an_interface_without_address_is_nothing() {
        assert_eq!(local_cidr(&InterfaceDetails::default()), None);
    }

    #[test]
    fn mask_to_prefix_rejects_a_non_contiguous_mask() {
        // 255.0.255.0 n'est pas un masque : le laisser passer produirait un
        // préfixe faux, plus difficile à repérer qu'une absence.
        assert_eq!(mask_to_prefix("255.0.255.0"), None);
        assert_eq!(mask_to_prefix("255.255.255.0"), Some(24));
        assert_eq!(mask_to_prefix("0.0.0.0"), Some(0));
    }
}
