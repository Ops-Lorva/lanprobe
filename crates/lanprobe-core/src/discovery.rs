use serde::Serialize;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::str::FromStr;
use super::proc::{async_cmd, sync_cmd};

#[derive(Debug, Serialize, Clone, Default)]
pub struct DiscoveredHost {
    pub ip: String,
    pub hostname: Option<String>,
    pub mac: Option<String>,
    pub vendor: Option<String>,
    pub latency_ms: Option<u64>,
}

pub fn parse_cidr(cidr: &str) -> Result<(u32, u32), String> {
    let parts: Vec<&str> = cidr.split('/').collect();
    if parts.len() != 2 {
        return Err("Format CIDR requis: 192.168.1.0/24".to_string());
    }
    let ip = Ipv4Addr::from_str(parts[0]).map_err(|e| e.to_string())?;
    let prefix: u32 = parts[1].parse().map_err(|_| "Préfixe invalide".to_string())?;
    if prefix > 32 { return Err("Préfixe > 32".to_string()); }
    let ip_int = u32::from(ip);
    let mask = if prefix == 0 { 0 } else { !0u32 << (32 - prefix) };
    let network = ip_int & mask;
    let first = network + 1;
    let last = (network | !mask) - 1;
    Ok((first, last))
}


/// L'interface par laquelle un scan doit voir le réseau.
///
/// Le nom sert à `ip neigh show dev …` et `arp -i …` ; l'IP sert à Windows,
/// dont `arp -a` désigne les interfaces par leur adresse, pas par leur nom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanInterface {
    pub name: String,
    pub ip: Option<Ipv4Addr>,
}

/// L'interface de scan correspondant au nom sélectionné dans l'UI, ou `None`
/// quand aucune interface n'est imposée.
pub fn scan_interface(selected: Option<&str>) -> Option<ScanInterface> {
    let name = selected?;
    Some(scan_interface_from(
        name,
        &crate::interfaces::get_interface_details(name),
    ))
}

/// Sur macOS le nom sélectionné est un service networksetup (« Wi-Fi ») :
/// `arp -i` et `ip neigh dev` attendent l'ifname système (« en0 »).
fn scan_interface_from(
    selected: &str,
    details: &crate::interfaces::InterfaceDetails,
) -> ScanInterface {
    ScanInterface {
        name: details
            .bsd_name
            .clone()
            .unwrap_or_else(|| selected.to_string()),
        ip: details.ip.as_deref().and_then(|s| s.parse().ok()),
    }
}

/// Arguments de `ip neigh`, restreints à l'interface choisie.
fn neigh_args(iface: Option<&ScanInterface>) -> Vec<&str> {
    match iface {
        Some(i) => vec!["neigh", "show", "dev", i.name.as_str()],
        None => vec!["neigh", "show"],
    }
}

/// Arguments de `arp` (BSD / net-tools), restreints à l'interface choisie.
fn bsd_arp_args(iface: Option<&ScanInterface>) -> Vec<&str> {
    match iface {
        Some(i) => vec!["-an", "-i", i.name.as_str()],
        None => vec!["-an"],
    }
}

/// Lit la table ARP pour récupérer les hôtes déjà connus sur le réseau local.
/// Retourne une map ip -> mac.
///
/// **Toujours restreinte à l'interface choisie.** Un Wi-Fi de technicien en
/// 192.168.1.0/24 et un VLAN client dans le même plan, c'est banal : sans
/// filtre, le scan du VLAN affiche avec MAC et constructeur des machines que
/// l'interface choisie n'a jamais vues, indiscernables des vrais hôtes.
///
/// - Windows : `arp -a`, filtré sur le bloc de l'IP de l'interface (`arp`
///   n'accepte pas de nom d'interface, seulement l'adresse locale).
/// - macOS   : `arp -an -i <iface>`, plus filtrage du `on <iface>` de la
///   sortie — la restriction ne dépend donc pas du support de `-i`.
/// - Linux   : `ip neigh show dev <iface>` (iproute2, toujours installé)
///   avec repli `arp -an -i <iface>` — `net-tools` n'est plus installé par
///   défaut sur Debian/Ubuntu récents, donc `arp` peut être absent.
pub async fn read_arp_table(iface: Option<&ScanInterface>) -> HashMap<String, String> {
    let mut map = HashMap::new();

    #[cfg(target_os = "windows")]
    {
        let out = async_cmd("arp").arg("-a").output().await;
        if let Ok(o) = out {
            let text = String::from_utf8_lossy(&o.stdout).into_owned();
            parse_windows_arp(&text, &mut map, iface.and_then(|i| i.ip));
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(o) = async_cmd("arp").args(bsd_arp_args(iface)).output().await {
            let text = String::from_utf8_lossy(&o.stdout).into_owned();
            parse_bsd_arp(&text, &mut map, iface.map(|i| i.name.as_str()));
        }
    }

    #[cfg(target_os = "linux")]
    {
        // `ip neigh show dev eth0` — format :
        // "192.168.1.1 dev eth0 lladdr aa:bb:cc:dd:ee:ff REACHABLE"
        if let Ok(o) = async_cmd("ip").args(neigh_args(iface)).output().await {
            let text = String::from_utf8_lossy(&o.stdout).into_owned();
            parse_ip_neigh(&text, &mut map, iface.map(|i| i.name.as_str()));
        }
        if map.is_empty() {
            if let Ok(o) = async_cmd("arp").args(bsd_arp_args(iface)).output().await {
                let text = String::from_utf8_lossy(&o.stdout).into_owned();
                parse_bsd_arp(&text, &mut map, iface.map(|i| i.name.as_str()));
            }
        }
    }

    map
}

/// Parse `ip neigh show`. Le filtre est redondant avec `dev <iface>` mais
/// couvre les sorties où la restriction n'a pas été appliquée.
#[allow(dead_code)]
fn parse_ip_neigh(text: &str, map: &mut HashMap<String, String>, iface: Option<&str>) {
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some(ip) = parts.first().filter(|p| p.contains('.')) else { continue };
        if let Some(want) = iface {
            let dev = parts
                .iter()
                .position(|p| *p == "dev")
                .and_then(|i| parts.get(i + 1));
            if dev != Some(&want) {
                continue;
            }
        }
        let mac = parts
            .iter()
            .position(|p| *p == "lladdr")
            .and_then(|i| parts.get(i + 1));
        if let Some(mac) = mac {
            if mac.contains(':') && mac.len() == 17 {
                map.insert(ip.to_string(), mac.to_string());
            }
        }
    }
}

/// Parse `arp -a` de Windows, bloc par bloc. Chaque bloc est introduit par
/// `Interface: <ip locale> --- 0x…` : c'est le seul moyen de rattacher une
/// entrée à une interface, `arp` ne prenant pas de nom d'interface.
#[allow(dead_code)]
fn parse_windows_arp(text: &str, map: &mut HashMap<String, String>, iface_ip: Option<Ipv4Addr>) {
    let mut dans_le_bon_bloc = iface_ip.is_none();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Interface:") {
            let bloc_ip = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<Ipv4Addr>().ok());
            dans_le_bon_bloc = match iface_ip {
                None => true,
                Some(want) => bloc_ip == Some(want),
            };
            continue;
        }
        if !dans_le_bon_bloc {
            continue;
        }
        // "  192.168.1.1   11-22-33-44-55-66   dynamic"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && parts[0].contains('.') && parts[1].contains('-') {
            map.insert(parts[0].to_string(), parts[1].replace('-', ":"));
        }
    }
}

#[allow(dead_code)]
fn parse_bsd_arp(text: &str, map: &mut HashMap<String, String>, iface: Option<&str>) {
    // Format BSD / macOS : "? (192.168.1.1) at 11:22:33:44:55:66 on en0 [ethernet]"
    for line in text.lines() {
        let Some(ip_start) = line.find('(') else { continue; };
        let Some(ip_end) = line.find(')') else { continue; };
        let ip = line[ip_start + 1..ip_end].to_string();
        if !ip.contains('.') { continue; }
        // Après `)` on a `" at <mac> on <iface>..."`. On cherche le token qui
        // suit directement `at`, pas `nth(1)` qui tombait sur `at` lui-même
        // puisque `)` compte pour un token quand split_whitespace remonte
        // depuis ip_end (vs ip_end + 1).
        let mut toks = line[ip_end + 1..].split_whitespace();
        let mac = loop {
            match toks.next() {
                Some("at") => break toks.next().unwrap_or("").to_string(),
                Some(_) => continue,
                None => break String::new(),
            }
        };
        // `on <iface>` : on ne garde que les voisins vus par l'interface
        // choisie, même si `arp -i` n'a pas restreint la sortie.
        if let Some(want) = iface {
            let mut reste = line[ip_end + 1..].split_whitespace();
            let dev = loop {
                match reste.next() {
                    Some("on") => break reste.next(),
                    Some(_) => continue,
                    None => break None,
                }
            };
            if dev != Some(want) {
                continue;
            }
        }
        if mac.contains(':') && mac != "(incomplete)" && mac.len() >= 11 {
            // Normalise en "aa:bb:cc:dd:ee:ff" — BSD arp raccourcit les octets
            // à 0 : "1:2:3:4:5:6" au lieu de "01:02:03:04:05:06".
            let normalized: Vec<String> = mac.split(':')
                .map(|b| if b.len() == 1 { format!("0{}", b) } else { b.to_string() })
                .collect();
            if normalized.len() == 6 {
                map.insert(ip, normalized.join(":"));
            }
        }
    }
}

/// Reverse DNS lookup — utilise `getnameinfo` via dns-lookup (fonctionne sur
/// Linux/Windows). Sur macOS on passe d'abord par `dscacheutil` pour que la
/// requête traverse mDNSResponder et résolve les noms Bonjour `*.local` :
/// `getnameinfo` seul interroge le DNS unicast et ne voit pas le LAN.
pub async fn get_hostname(ip: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        if let Some(h) = macos_reverse_lookup(ip).await {
            return Some(h);
        }
    }
    let ip_addr: std::net::IpAddr = ip.parse().ok()?;
    let ip_owned = ip.to_string();
    tokio::task::spawn_blocking(move || {
        dns_lookup::lookup_addr(&ip_addr).ok()
    })
    .await
    .ok()
    .flatten()
    .filter(|h| !h.is_empty() && h != &ip_owned)
}

#[cfg(target_os = "macos")]
async fn macos_reverse_lookup(ip: &str) -> Option<String> {
    // `dscacheutil -q host -a ip <ip>` passe par le resolver système
    // (mDNSResponder), donc il voit les noms Bonjour `*.local` publiés sur
    // le LAN — contrairement à `getnameinfo` qui ne query que le DNS unicast.
    // Sortie type :
    //   name: router.local
    //   alias:
    //   ip_address: 192.168.1.1
    let out = async_cmd("dscacheutil")
        .args(["-q", "host", "-a", "ip", ip])
        .output()
        .await
        .ok()?;
    if !out.status.success() { return None; }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("name:") {
            let name = rest.trim();
            if !name.is_empty() && name != ip {
                return Some(name.to_string());
            }
        }
    }
    None
}

pub fn get_local_network_cidr() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let text = sync_cmd("ip").args(["route"]).output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        return text.lines()
            .filter(|l| !l.starts_with("default") && l.contains('/'))
            .filter_map(|l| l.split_whitespace().next().map(String::from))
            .filter(|s| s.contains('.'))
            .next();
    }
    #[cfg(target_os = "macos")]
    {
        let iface_text = sync_cmd("route").args(["get", "default"]).output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let iface = iface_text.lines()
            .find(|l| l.contains("interface:"))?
            .split(':').nth(1)?
            .trim().to_string();
        let ifconfig_text = sync_cmd("ifconfig").arg(&iface).output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let mut ip_str = None;
        let mut prefix = None;
        for line in ifconfig_text.lines() {
            let line = line.trim();
            if line.starts_with("inet ") && !line.starts_with("inet6") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                ip_str = parts.get(1).map(|s| s.to_string());
                if let Some(idx) = parts.iter().position(|p| *p == "prefixlen") {
                    prefix = parts.get(idx + 1).and_then(|p| p.parse::<u32>().ok());
                } else if let Some(idx) = parts.iter().position(|p| *p == "netmask") {
                    if let Some(mask_hex) = parts.get(idx + 1) {
                        let mask_hex = mask_hex.trim_start_matches("0x");
                        if let Ok(mask) = u32::from_str_radix(mask_hex, 16) {
                            prefix = Some(mask.count_ones());
                        }
                    }
                }
                break;
            }
        }
        let ip = ip_str?;
        let prefix = prefix?;
        let parts: Vec<u8> = ip.split('.').filter_map(|p| p.parse().ok()).collect();
        if parts.len() != 4 { return None; }
        let ip_int = u32::from_be_bytes([parts[0], parts[1], parts[2], parts[3]]);
        let mask = if prefix == 0 { 0 } else { !0u32 << (32 - prefix) };
        let network = ip_int & mask;
        let net = std::net::Ipv4Addr::from(network);
        return Some(format!("{}/{}", net, prefix));
    }
    #[cfg(target_os = "windows")]
    {
        let text = sync_cmd("ipconfig").output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        // `ipconfig` liste les adaptateurs par bloc ; on ignore les vEthernet
        // WSL/Hyper-V qui arrivent souvent en tête et font scanner 172.x.
        let mut ip_str = None;
        let mut mask_str = None;
        let mut skip_block = false;
        for line in text.lines() {
            if !line.starts_with(' ') && !line.trim().is_empty() {
                // Nouveau bloc adaptateur
                let header = line.to_lowercase();
                skip_block = header.contains("wsl") || header.contains("vethernet") || header.contains("hyper-v") || header.contains("loopback");
                ip_str = None;
                mask_str = None;
                continue;
            }
            if skip_block { continue; }
            let l = line.trim();
            if l.contains("IPv4 Address") && ip_str.is_none() {
                ip_str = l.splitn(2, ':').nth(1).map(|s| s.trim().trim_end_matches('(').to_string());
            } else if l.contains("Subnet Mask") && ip_str.is_some() && mask_str.is_none() {
                mask_str = l.splitn(2, ':').nth(1).map(|s| s.trim().to_string());
                break;
            }
        }
        let ip = ip_str?;
        let mask = mask_str?;
        let ip_parts: Vec<u8> = ip.split('.').filter_map(|p| p.parse().ok()).collect();
        let mask_parts: Vec<u8> = mask.split('.').filter_map(|p| p.parse().ok()).collect();
        if ip_parts.len() != 4 || mask_parts.len() != 4 { return None; }
        let ip_int = u32::from_be_bytes([ip_parts[0], ip_parts[1], ip_parts[2], ip_parts[3]]);
        let mask_int = u32::from_be_bytes([mask_parts[0], mask_parts[1], mask_parts[2], mask_parts[3]]);
        let prefix = mask_int.count_ones();
        let network = ip_int & mask_int;
        let net = std::net::Ipv4Addr::from(network);
        return Some(format!("{}/{}", net, prefix));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cidr_24() {
        let (first, last) = parse_cidr("192.168.1.0/24").unwrap();
        let first_ip = Ipv4Addr::from(first);
        let last_ip = Ipv4Addr::from(last);
        assert_eq!(first_ip, Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(last_ip, Ipv4Addr::new(192, 168, 1, 254));
    }

    #[test]
    fn test_parse_cidr_invalid() {
        assert!(parse_cidr("not-a-cidr").is_err());
        assert!(parse_cidr("192.168.1.0/33").is_err());
    }

    #[test]
    fn sans_interface_selectionnee_le_scan_n_est_pas_restreint() {
        assert_eq!(scan_interface(None), None);
    }

    #[test]
    fn le_scan_utilise_le_nom_bsd_car_c_est_lui_que_arp_connait() {
        // Sur macOS l'interface sélectionnée est un service networksetup
        // (« Wi-Fi ») ; `arp -i` attend « en0 ».
        let details = crate::interfaces::InterfaceDetails {
            name: "Wi-Fi".to_string(),
            ip: Some("192.168.1.10".to_string()),
            bsd_name: Some("en0".to_string()),
            ..Default::default()
        };
        assert_eq!(
            scan_interface_from("Wi-Fi", &details),
            ScanInterface {
                name: "en0".to_string(),
                ip: Some(Ipv4Addr::new(192, 168, 1, 10)),
            }
        );
    }

    #[test]
    fn sans_nom_bsd_le_scan_garde_le_nom_selectionne() {
        let details = crate::interfaces::InterfaceDetails {
            name: "eth0".to_string(),
            ip: Some("10.0.30.12".to_string()),
            ..Default::default()
        };
        assert_eq!(
            scan_interface_from("eth0", &details),
            ScanInterface {
                name: "eth0".to_string(),
                ip: Some(Ipv4Addr::new(10, 0, 30, 12)),
            }
        );
    }

    fn scope(name: &str) -> ScanInterface {
        ScanInterface {
            name: name.to_string(),
            ip: Some(Ipv4Addr::new(192, 168, 1, 10)),
        }
    }

    #[test]
    fn ip_neigh_est_restreint_a_l_interface_choisie() {
        assert_eq!(
            neigh_args(Some(&scope("eth0"))),
            vec!["neigh", "show", "dev", "eth0"]
        );
    }

    #[test]
    fn sans_interface_choisie_ip_neigh_voit_tout() {
        assert_eq!(neigh_args(None), vec!["neigh", "show"]);
    }

    #[test]
    fn arp_bsd_est_restreint_a_l_interface_choisie() {
        assert_eq!(bsd_arp_args(Some(&scope("en0"))), vec!["-an", "-i", "en0"]);
        assert_eq!(bsd_arp_args(None), vec!["-an"]);
    }

    #[test]
    fn la_table_bsd_ecarte_les_voisins_d_une_autre_interface() {
        // Le Wi-Fi du technicien et le VLAN client sont tous deux en
        // 192.168.1.0/24 : sans filtre, les machines du Wi-Fi s'affichent
        // comme des hôtes découverts du VLAN.
        let text = "? (192.168.1.1) at 11:22:33:44:55:66 on en0 [ethernet]\n\
                    ? (192.168.1.2) at aa:bb:cc:dd:ee:ff on en1 [ethernet]\n";
        let mut map = HashMap::new();
        parse_bsd_arp(text, &mut map, Some("en0"));
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("192.168.1.1").map(String::as_str), Some("11:22:33:44:55:66"));
    }

    #[test]
    fn sans_filtre_la_table_bsd_garde_toutes_les_interfaces() {
        let text = "? (192.168.1.1) at 11:22:33:44:55:66 on en0 [ethernet]\n\
                    ? (192.168.1.2) at aa:bb:cc:dd:ee:ff on en1 [ethernet]\n";
        let mut map = HashMap::new();
        parse_bsd_arp(text, &mut map, None);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn ip_neigh_ecarte_les_voisins_d_une_autre_interface() {
        // `ip neigh show dev eth0` filtre déjà côté noyau, mais le repli
        // `arp -an` et les sorties non filtrées doivent l'être aussi.
        let text = "192.168.1.1 dev eth0 lladdr 11:22:33:44:55:66 REACHABLE\n\
                    192.168.1.2 dev wlan0 lladdr aa:bb:cc:dd:ee:ff REACHABLE\n";
        let mut map = HashMap::new();
        parse_ip_neigh(text, &mut map, Some("eth0"));
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("192.168.1.1"));
    }

    #[test]
    fn la_table_windows_ne_garde_que_le_bloc_de_l_interface() {
        // `arp -a` liste un bloc par interface, en tête son IP locale.
        let text = "\r\nInterface: 192.168.1.10 --- 0x5\r\n\
                    Internet Address      Physical Address      Type\r\n\
                    192.168.1.1           11-22-33-44-55-66     dynamic\r\n\
                    \r\nInterface: 10.0.0.5 --- 0x9\r\n\
                    Internet Address      Physical Address      Type\r\n\
                    192.168.1.2           aa-bb-cc-dd-ee-ff     dynamic\r\n";
        let mut map = HashMap::new();
        parse_windows_arp(text, &mut map, Some(Ipv4Addr::new(192, 168, 1, 10)));
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("192.168.1.1").map(String::as_str), Some("11:22:33:44:55:66"));
    }

    #[test]
    fn sans_filtre_la_table_windows_garde_tous_les_blocs() {
        let text = "\r\nInterface: 192.168.1.10 --- 0x5\r\n\
                    192.168.1.1           11-22-33-44-55-66     dynamic\r\n\
                    \r\nInterface: 10.0.0.5 --- 0x9\r\n\
                    192.168.1.2           aa-bb-cc-dd-ee-ff     dynamic\r\n";
        let mut map = HashMap::new();
        parse_windows_arp(text, &mut map, None);
        assert_eq!(map.len(), 2);
    }
}
