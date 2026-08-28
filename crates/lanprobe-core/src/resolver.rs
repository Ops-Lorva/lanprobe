//! Résolution DNS liée à l'interface auditée.
//!
//! `getaddrinfo` interroge le résolveur du **système**, par la route par
//! défaut : le DNS du VLAN client peut être mort, le Wi-Fi du technicien
//! répondra très bien et la pastille DNS restera verte. La mesure décrirait
//! alors le poste du technicien, pas le réseau audité.
//!
//! Quand une interface est sélectionnée, on émet donc les requêtes **depuis
//! son IP** et **vers ses serveurs à elle**. Quand aucune interface n'est
//! sélectionnée, le résolveur système reste légitime : c'est bien la route
//! par défaut qu'on mesure.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use hickory_resolver::config::{ConnectionConfig, NameServerConfig, ResolverConfig};

use crate::interfaces::InterfaceDetails;

/// D'où sortent les requêtes DNS d'une mesure, et vers qui.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsBinding {
    /// IP source imposée aux paquets DNS.
    pub src: Ipv4Addr,
    /// Serveurs à interroger — ceux de l'interface, pas ceux du système.
    pub servers: Vec<IpAddr>,
}

/// Serveurs DNS utilisables pour interroger le réseau d'une interface.
///
/// Les adresses de loopback sont écartées : `127.0.0.53` est le stub de
/// `systemd-resolved`, c'est-à-dire exactement le résolveur système qu'on
/// cherche à contourner — et il est de toute façon injoignable depuis l'IP
/// d'une interface physique.
pub fn dns_servers_for(details: &InterfaceDetails) -> Vec<IpAddr> {
    merge_servers(per_link_servers(&details.name), &details.dns)
}

/// Les serveurs du lien décrivent le réseau audité ; ceux déclarés
/// globalement ne sont qu'un pis-aller.
fn merge_servers(per_link: Vec<IpAddr>, declared: &[String]) -> Vec<IpAddr> {
    if !per_link.is_empty() {
        return per_link;
    }
    declared
        .iter()
        .filter_map(|s| s.trim().parse::<IpAddr>().ok())
        .filter(|ip| !ip.is_loopback())
        .collect()
}

/// Serveurs DNS que systemd-resolved associe à CE lien.
///
/// Sous Linux, `/etc/resolv.conf` est global et pointe le plus souvent sur
/// le stub `127.0.0.53` — c'est-à-dire sur le résolveur système, celui qu'on
/// cherche justement à contourner. `resolvectl dns <iface>` rend, lui, les
/// serveurs réellement appris sur cette interface.
#[cfg(target_os = "linux")]
fn per_link_servers(iface: &str) -> Vec<IpAddr> {
    let out = crate::proc::sync_cmd("resolvectl")
        .args(["dns", iface])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            parse_resolvectl_dns(&String::from_utf8_lossy(&o.stdout))
        }
        _ => Vec::new(),
    }
}

/// Sur macOS (`networksetup -getdnsservers`) et Windows
/// (`Get-DnsClientServerAddress -InterfaceAlias`), la liste rendue par
/// `get_interface_details` est déjà celle de l'interface : rien à ajouter.
#[cfg(not(target_os = "linux"))]
fn per_link_servers(_iface: &str) -> Vec<IpAddr> {
    Vec::new()
}

/// Parse `Link 2 (enp3s0): 10.0.30.1 10.0.30.2`.
fn parse_resolvectl_dns(text: &str) -> Vec<IpAddr> {
    text.lines()
        .filter(|l| l.trim_start().starts_with("Link"))
        .filter_map(|l| l.split_once(':'))
        .flat_map(|(_, servers)| servers.split_whitespace())
        .filter_map(|s| s.parse::<IpAddr>().ok())
        .filter(|ip| !ip.is_loopback())
        .collect()
}

/// Le liage DNS de l'interface, ou `None` si la mesure est impossible :
/// pas d'IPv4, ou aucun serveur DNS utilisable déclaré. Dans ce cas la
/// sonde doit reporter un échec — pas se rabattre en silence sur le
/// résolveur système.
pub fn binding_for(details: &InterfaceDetails) -> Option<DnsBinding> {
    let src: Ipv4Addr = details.ip.as_deref()?.parse().ok()?;
    let servers = dns_servers_for(details);
    if servers.is_empty() {
        return None;
    }
    Some(DnsBinding { src, servers })
}

/// Ce que la sonde doit faire pour mesurer le DNS, selon l'interface choisie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsPlan {
    /// Aucune interface imposée : le résolveur système mesure bien la route
    /// par défaut, qui est ce qu'on veut mesurer.
    System,
    /// Interface imposée : requêtes émises depuis son IP, vers ses serveurs.
    Bound(DnsBinding),
    /// Interface imposée mais inexploitable (pas d'IPv4, ou aucun serveur
    /// DNS déclaré ailleurs que sur la loopback). La mesure échoue — elle ne
    /// se rabat pas sur le résolveur système.
    Impossible,
}

pub fn dns_plan(details: Option<&InterfaceDetails>) -> DnsPlan {
    match details {
        None => DnsPlan::System,
        Some(d) => match binding_for(d) {
            Some(b) => DnsPlan::Bound(b),
            None => DnsPlan::Impossible,
        },
    }
}

/// Config hickory qui émet depuis l'IP choisie vers les serveurs choisis.
pub fn bound_config(binding: &DnsBinding) -> ResolverConfig {
    let bind_addr = Some(SocketAddr::new(IpAddr::V4(binding.src), 0));
    let servers = binding
        .servers
        .iter()
        .map(|ip| {
            let connections = [ConnectionConfig::udp(), ConnectionConfig::tcp()]
                .into_iter()
                .map(|mut c| {
                    c.bind_addr = bind_addr;
                    c
                })
                .collect();
            NameServerConfig::new(*ip, true, connections)
        })
        .collect();
    ResolverConfig::from_parts(None, Vec::new(), servers)
}

/// Résout un nom. `binding = None` → résolveur système (route par défaut).
pub async fn lookup(
    host: &str,
    binding: Option<&DnsBinding>,
    timeout: Duration,
) -> Result<Vec<IpAddr>, String> {
    match binding {
        Some(b) => lookup_with_config(bound_config(b), host, timeout).await,
        None => lookup_system(host, timeout).await,
    }
}

/// Résolution par le résolveur du système (`getaddrinfo`). Légitime tant
/// qu'aucune interface n'est imposée : c'est la route par défaut qu'on mesure.
async fn lookup_system(host: &str, timeout: Duration) -> Result<Vec<IpAddr>, String> {
    let host = host.to_string();
    let joined = tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            use std::net::ToSocketAddrs;
            (host.as_str(), 80u16)
                .to_socket_addrs()
                .map(|it| it.map(|s| s.ip()).collect::<Vec<_>>())
                .map_err(|e| e.to_string())
        }),
    )
    .await
    .map_err(|_| "délai dépassé".to_string())?
    .map_err(|e| e.to_string())?;
    joined
}

/// Résolveur que `reqwest` utilisera pour les mesures HTTP. Sans lui, la
/// connexion partirait bien de l'interface choisie mais le nom serait résolu
/// par le résolveur système — la moitié de la mesure décrirait encore le
/// poste du technicien.
pub struct BoundHttpResolver {
    resolver: std::sync::Arc<
        hickory_resolver::Resolver<hickory_resolver::net::runtime::TokioRuntimeProvider>,
    >,
}

impl BoundHttpResolver {
    pub fn new(binding: &DnsBinding, timeout: Duration) -> Option<Self> {
        Self::build(bound_config(binding), timeout)
    }

    /// Même chose, en visant un port choisi — les tests montent un faux
    /// serveur DNS, qui ne tourne pas sur le port 53.
    pub fn with_port(binding: &DnsBinding, port: u16, timeout: Duration) -> Option<Self> {
        let mut config = bound_config(binding);
        for ns in config.name_servers.iter_mut() {
            for conn in ns.connections.iter_mut() {
                conn.port = port;
            }
        }
        Self::build(config, timeout)
    }

    fn build(config: ResolverConfig, timeout: Duration) -> Option<Self> {
        let mut builder = hickory_resolver::Resolver::builder_with_config(
            config,
            hickory_resolver::net::runtime::TokioRuntimeProvider::default(),
        );
        {
            let opts = builder.options_mut();
            opts.timeout = timeout;
            opts.attempts = 0;
            opts.use_hosts_file = hickory_resolver::config::ResolveHosts::Never;
        }
        builder
            .build()
            .ok()
            .map(|resolver| Self {
                resolver: std::sync::Arc::new(resolver),
            })
    }
}

impl reqwest::dns::Resolve for BoundHttpResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let resolver = self.resolver.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            let lookup = resolver
                .lookup_ip(host.as_str())
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
            let addrs: reqwest::dns::Addrs =
                Box::new(lookup.iter().collect::<Vec<_>>().into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

/// Résolution avec une config hickory explicite. Aucun repli : si ces
/// serveurs-là ne répondent pas, la mesure échoue.
pub async fn lookup_with_config(
    config: ResolverConfig,
    host: &str,
    timeout: Duration,
) -> Result<Vec<IpAddr>, String> {
    let mut builder = hickory_resolver::Resolver::builder_with_config(
        config,
        hickory_resolver::net::runtime::TokioRuntimeProvider::default(),
    );
    {
        let opts = builder.options_mut();
        opts.timeout = timeout;
        // Une seule tentative : la mesure doit rendre la main dans le délai
        // annoncé, pas le multiplier par le nombre de reprises.
        opts.attempts = 0;
        // Ni cache, ni /etc/hosts : on mesure ce que le serveur du réseau
        // audité répond, maintenant.
        opts.cache_size = 0;
        opts.use_hosts_file = hickory_resolver::config::ResolveHosts::Never;
    }
    let resolver = builder.build().map_err(|e| e.to_string())?;
    let lookup = tokio::time::timeout(timeout, resolver.lookup_ip(host))
        .await
        .map_err(|_| "délai dépassé".to_string())?
        .map_err(|e| e.to_string())?;
    Ok(lookup.iter().collect())
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn details(ip: Option<&str>, dns: &[&str]) -> InterfaceDetails {
        InterfaceDetails {
            name: "eth0".to_string(),
            ip: ip.map(String::from),
            dns: dns.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn les_serveurs_retenus_sont_ceux_declares_sur_l_interface() {
        let d = details(Some("10.0.30.12"), &["10.0.30.1", "10.0.30.2"]);
        assert_eq!(
            dns_servers_for(&d),
            vec![
                IpAddr::V4(Ipv4Addr::new(10, 0, 30, 1)),
                IpAddr::V4(Ipv4Addr::new(10, 0, 30, 2)),
            ]
        );
    }

    #[test]
    fn le_stub_de_loopback_n_est_pas_un_serveur_du_reseau_audite() {
        let d = details(Some("10.0.30.12"), &["127.0.0.53", "10.0.30.1"]);
        assert_eq!(
            dns_servers_for(&d),
            vec![IpAddr::V4(Ipv4Addr::new(10, 0, 30, 1))]
        );
    }

    #[test]
    fn une_adresse_illisible_est_ignoree() {
        let d = details(Some("10.0.30.12"), &["pas-une-ip", "10.0.30.1"]);
        assert_eq!(
            dns_servers_for(&d),
            vec![IpAddr::V4(Ipv4Addr::new(10, 0, 30, 1))]
        );
    }

    #[test]
    fn les_serveurs_du_lien_priment_sur_le_resolv_conf_global() {
        // Sous Linux `/etc/resolv.conf` est global : quand systemd-resolved
        // sait quels serveurs porte CE lien, ce sont eux qui décrivent le
        // réseau audité.
        let per_link = vec![IpAddr::V4(Ipv4Addr::new(10, 0, 30, 1))];
        let declares = vec!["8.8.8.8".to_string()];
        assert_eq!(merge_servers(per_link.clone(), &declares), per_link);
    }

    #[test]
    fn sans_serveur_de_lien_on_retombe_sur_ce_que_declare_l_interface() {
        let declares = vec!["127.0.0.53".to_string(), "10.0.30.1".to_string()];
        assert_eq!(
            merge_servers(Vec::new(), &declares),
            vec![IpAddr::V4(Ipv4Addr::new(10, 0, 30, 1))]
        );
    }

    #[test]
    fn resolvectl_donne_les_serveurs_du_lien() {
        let sortie = "Link 2 (enp3s0): 10.0.30.1 10.0.30.2\n";
        assert_eq!(
            parse_resolvectl_dns(sortie),
            vec![
                IpAddr::V4(Ipv4Addr::new(10, 0, 30, 1)),
                IpAddr::V4(Ipv4Addr::new(10, 0, 30, 2)),
            ]
        );
    }

    #[test]
    fn resolvectl_sans_serveur_ne_donne_rien() {
        assert!(parse_resolvectl_dns("Link 2 (enp3s0):\n").is_empty());
    }

    #[test]
    fn resolvectl_ignore_le_stub_de_loopback() {
        assert!(parse_resolvectl_dns("Link 2 (enp3s0): 127.0.0.53\n").is_empty());
    }

    #[test]
    fn le_liage_reprend_l_ip_et_les_serveurs_de_l_interface() {
        let d = details(Some("10.0.30.12"), &["10.0.30.1"]);
        let b = binding_for(&d).expect("interface complète");
        assert_eq!(b.src, Ipv4Addr::new(10, 0, 30, 12));
        assert_eq!(b.servers, vec![IpAddr::V4(Ipv4Addr::new(10, 0, 30, 1))]);
    }

    #[test]
    fn sans_ipv4_il_n_y_a_pas_de_liage_possible() {
        let d = details(None, &["10.0.30.1"]);
        assert!(binding_for(&d).is_none());
    }

    #[test]
    fn sans_serveur_utilisable_il_n_y_a_pas_de_liage_possible() {
        // Mieux vaut un échec visible qu'une mesure qui décrit le résolveur
        // du technicien.
        let d = details(Some("10.0.30.12"), &["127.0.0.53"]);
        assert!(binding_for(&d).is_none());
    }

    #[test]
    fn sans_interface_choisie_le_resolveur_systeme_reste_legitime() {
        assert_eq!(dns_plan(None), DnsPlan::System);
    }

    #[test]
    fn une_interface_complete_impose_son_propre_resolveur() {
        let d = details(Some("10.0.30.12"), &["10.0.30.1"]);
        assert_eq!(
            dns_plan(Some(&d)),
            DnsPlan::Bound(DnsBinding {
                src: Ipv4Addr::new(10, 0, 30, 12),
                servers: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 30, 1))],
            })
        );
    }

    #[test]
    fn une_interface_sans_resolveur_utilisable_rend_la_mesure_impossible() {
        // Surtout pas `System` : ce serait mesurer le réseau du technicien.
        let d = details(Some("10.0.30.12"), &[]);
        assert_eq!(dns_plan(Some(&d)), DnsPlan::Impossible);
    }

    #[test]
    fn chaque_connexion_du_resolveur_part_de_l_ip_choisie() {
        let b = DnsBinding {
            src: Ipv4Addr::new(10, 0, 30, 12),
            servers: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 30, 1))],
        };
        let cfg = bound_config(&b);
        let servers = cfg.name_servers();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].ip, IpAddr::V4(Ipv4Addr::new(10, 0, 30, 1)));
        assert!(!servers[0].connections.is_empty());
        for conn in &servers[0].connections {
            assert_eq!(
                conn.bind_addr,
                Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 30, 12)), 0)),
                "toute connexion DNS doit sortir de l'IP de l'interface"
            );
        }
    }

    /// Même vérification pour le résolveur que reqwest utilisera : sans lui,
    /// `probe_http` résoudrait son nom de cible par le résolveur système même
    /// quand la connexion TCP, elle, part de la bonne interface.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn le_resolveur_http_sort_aussi_de_l_ip_choisie() {
        use reqwest::dns::Resolve;

        let serveur = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = serveur.local_addr().unwrap().port();
        let vu = tokio::spawn(async move {
            let mut buf = [0u8; 512];
            let (_, from) = serveur.recv_from(&mut buf).await.unwrap();
            from
        });

        let binding = DnsBinding {
            src: Ipv4Addr::new(127, 0, 0, 2),
            servers: vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
        };
        let resolveur = BoundHttpResolver::with_port(&binding, port, Duration::from_millis(500))
            .expect("le résolveur lié se construit");
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            resolveur.resolve("cloudflare.com".parse().unwrap()),
        )
        .await;

        let from = tokio::time::timeout(Duration::from_secs(2), vu)
            .await
            .expect("le faux serveur DNS doit recevoir la requête")
            .unwrap();
        assert_eq!(from.ip(), IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)));
    }

    /// Le vrai test du liage : un faux serveur DNS sur la loopback, une
    /// requête liée à `127.0.0.2`, et on regarde l'adresse source du paquet
    /// reçu. Linux seul : les alias `127.0.0.0/8` n'existent pas ailleurs.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn la_requete_sort_bien_de_l_ip_choisie() {
        let serveur = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = serveur.local_addr().unwrap().port();
        let vu = tokio::spawn(async move {
            let mut buf = [0u8; 512];
            let (_, from) = serveur.recv_from(&mut buf).await.unwrap();
            from
        });

        let binding = DnsBinding {
            src: Ipv4Addr::new(127, 0, 0, 2),
            servers: vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
        };
        let mut cfg = bound_config(&binding);
        for ns in cfg.name_servers.iter_mut() {
            for conn in ns.connections.iter_mut() {
                conn.port = port;
            }
        }

        // La résolution échouera (personne ne répond) : ce qui compte est
        // l'adresse d'où le paquet est parti.
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            lookup_with_config(cfg, "cloudflare.com", Duration::from_millis(500)),
        )
        .await;

        let from = tokio::time::timeout(Duration::from_secs(2), vu)
            .await
            .expect("le faux serveur DNS doit recevoir la requête")
            .unwrap();
        assert_eq!(from.ip(), IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn un_serveur_muet_fait_echouer_la_mesure_sans_repli_systeme() {
        // Le serveur ne répond jamais : si `lookup` retombait sur le
        // résolveur système, cloudflare.com se résoudrait quand même et la
        // pastille DNS mentirait.
        let serveur = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = serveur.local_addr().unwrap().port();
        let binding = DnsBinding {
            src: Ipv4Addr::new(127, 0, 0, 1),
            servers: vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
        };
        let mut cfg = bound_config(&binding);
        for ns in cfg.name_servers.iter_mut() {
            for conn in ns.connections.iter_mut() {
                conn.port = port;
            }
        }

        let res = lookup_with_config(cfg, "cloudflare.com", Duration::from_millis(300)).await;
        assert!(res.is_err(), "la mesure doit échouer, pas se rabattre : {res:?}");
    }
}
