//! Les profils de scan de ports de la sonde, et leur alignement sur le hub
//! (contrat § 25).
//!
//! 🔴 **La sonde ne supprime JAMAIS un profil au motif qu'il est absent de la
//! liste reçue.** C'est peut-être celui qu'elle vient de créer : il est parti
//! au même battement, le hub l'ingérera, et il reviendra au suivant. Sans cette
//! règle, tout profil créé localement disparaîtrait une minute après sa
//! création.
//!
//! ⚠️ Les profils de **base** de la sonde ne vivent pas ici : ils sont dans le
//! code de son interface (`BUILTIN_PROFILES`), ne montent pas, ne descendent
//! pas, et c'est ce qui fait que l'écran de scan marche **sans hub**.

use serde::{Deserialize, Serialize};

/// Clé de la liste dans `app_config.json` — la même que celle qu'écrit
/// l'interface de la sonde, et c'est voulu : une seconde liste à côté de la
/// sienne donnerait deux vérités à l'écran.
pub const PROFILES_KEY: &str = "portscan_profiles";

/// Clé de la dernière révision **appliquée**.
///
/// 🔴 Ce n'est pas une date : le numéro vient du hub, la sonde le range et le
/// rend tel quel. Aucune horloge n'entre dans l'affaire — c'est déjà la raison
/// pour laquelle les changements de surveillance voyagent en ancienneté (§ 20).
pub const PROFILES_REV_KEY: &str = "portscan_profiles_rev";

/// Un profil tel que la sonde le garde.
///
/// ⚠️ `rest` ramasse tout ce que l'interface y met et que la sonde ne modélise
/// pas (`builtin`, un champ ajouté demain…). Sans lui, chaque alignement sur le
/// hub réécrirait la liste en **perdant** ces champs, silencieusement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub tcp_ports: Vec<u16>,
    /// ⚠️ **Le hub ne modélise pas l'UDP.** Cette liste survit donc à une
    /// réécriture venue du hub : il ne peut pas faire autorité sur un champ
    /// qu'il n'a pas, et l'écraser détruirait un réglage que rien ne pourrait
    /// reconstituer.
    #[serde(default)]
    pub udp_ports: Vec<u16>,
    /// Vrai quand le profil vient du hub. L'écran le dit, parce qu'une
    /// modification locale serait réécrite au battement suivant — laisser
    /// croire à une édition qui tiendra serait un mensonge d'interface.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub from_hub: bool,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, serde_json::Value>,
}

/// Un profil **de base** de l'application ? Ceux-là ne montent pas au hub
/// (décision 3) : ils vivent dans le code de l'interface, le hub sème les
/// mêmes, et les faire monter poserait autant de doublons dans la liste
/// commune de tout le parc.
///
/// ⚠️ Les deux critères, pas un seul : l'interface pose `builtin: true`, mais
/// une liste écrite par une version antérieure peut ne porter que le préfixe
/// d'identifiant. Se fier à un seul ferait monter la moitié des cas.
pub fn is_builtin(profile: &LocalProfile) -> bool {
    profile.id.starts_with("builtin:")
        || profile
            .rest
            .get("builtin")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

/// Un profil tel que le hub le rend au battement.
#[derive(Debug, Clone, Deserialize)]
pub struct HubProfile {
    #[serde(default)]
    pub profile_id: String,
    #[serde(default)]
    pub name: String,
    /// ⚠️ **Vide veut dire « garde ta liste »**, pas « ne scanne rien » : c'est
    /// déjà ainsi que la sonde traite une liste de ports absente, et confondre
    /// les deux ferait un scan complet là où on croyait restreindre.
    #[serde(default)]
    pub ports: Vec<u16>,
    /// Une date, c'est la suppression explicite. `None` = vivant.
    #[serde(default)]
    pub deleted_at: Option<i64>,
}

/// Aligne la liste locale sur ce que le hub vient de dire.
///
/// `replace` à vrai veut dire « ce n'est pas un delta, c'est l'état du hub » —
/// une sonde dont la révision était inconnue, trop ancienne, ou **en avance**
/// (hub restauré).
///
/// ⚠️ **Même avec `replace`, un profil créé ICI survit.** Le drapeau ne porte
/// que sur ce qui VIENT du hub : un profil local n'a peut-être pas encore été
/// ingéré, et le jeter serait exactement le défaut que le § 25 interdit.
pub fn merge(local: Vec<LocalProfile>, incoming: &[HubProfile], replace: bool) -> Vec<LocalProfile> {
    let mut out: Vec<LocalProfile> = local
        .into_iter()
        // Sur « remplace tout », les profils du hub sont reconstruits depuis la
        // liste reçue : celui qui n'y est plus a été supprimé, et sa pierre
        // tombale a pu être effacée depuis longtemps.
        .filter(|p| !(replace && p.from_hub))
        .collect();

    for entry in incoming {
        let id = entry.profile_id.trim();
        if id.is_empty() {
            continue;
        }
        if entry.deleted_at.is_some() {
            // 🔴 La suppression du hub gagne, et elle est le seul motif de
            // retrait. Sans elle, la sonde ne distinguerait pas « le hub ne
            // connaît pas ce profil » de « le hub l'a supprimé », et le
            // recréerait en le remontant au battement suivant.
            out.retain(|p| p.id != id);
            continue;
        }
        let previous = out.iter().position(|p| p.id == id);
        // Le hub fait autorité sur le nom et les ports TCP ; tout le reste de
        // la ligne locale est conservé.
        let mut next = match previous.map(|i| out.remove(i)) {
            Some(existing) => existing,
            None => LocalProfile {
                id: id.to_string(),
                name: String::new(),
                tcp_ports: Vec::new(),
                udp_ports: Vec::new(),
                from_hub: true,
                rest: serde_json::Map::new(),
            },
        };
        next.id = id.to_string();
        next.name = entry.name.clone();
        next.tcp_ports = entry.ports.clone();
        next.from_hub = true;
        out.push(next);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(id: &str, from_hub: bool) -> LocalProfile {
        LocalProfile {
            id: id.into(),
            name: format!("Profil {id}"),
            tcp_ports: vec![22],
            udp_ports: vec![],
            from_hub,
            rest: serde_json::Map::new(),
        }
    }

    fn from_hub(id: &str, ports: &[u16]) -> HubProfile {
        HubProfile {
            profile_id: id.into(),
            name: format!("Hub {id}"),
            ports: ports.to_vec(),
            deleted_at: None,
        }
    }

    #[test]
    fn un_profil_du_hub_est_ecrit_tel_quel_et_marque_comme_tel() {
        let out = merge(Vec::new(), &[from_hub("web", &[80, 443])], false);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Hub web");
        assert_eq!(out[0].tcp_ports, vec![80, 443]);
        // ⚠️ Le marquage n'est pas décoratif : c'est lui qui permet à l'écran
        // de dire qu'une modification locale sera réécrite au battement
        // suivant, et au « remplace tout » de ne jeter que ce qui vient du hub.
        assert!(out[0].from_hub);
    }

    #[test]
    fn une_pierre_tombale_retire_le_profil() {
        let out = merge(
            vec![local("web", true)],
            &[HubProfile {
                profile_id: "web".into(),
                name: "Web".into(),
                ports: vec![],
                deleted_at: Some(1_790_000_000),
            }],
            false,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn un_profil_local_absent_de_la_liste_recue_survit() {
        // 🔴 LA règle du § 25. Il vient peut-être d'être créé : il est parti au
        // même battement, le hub l'ingérera, et il reviendra au suivant. Le
        // retirer ferait disparaître tout profil créé localement une minute
        // après sa création.
        let out = merge(vec![local("cams", false)], &[from_hub("web", &[80])], false);
        let ids: Vec<&str> = out.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"cams"), "{ids:?}");
    }

    #[test]
    fn un_profil_monte_puis_redescendu_ne_fait_pas_un_doublon() {
        // 🔴 Ce qui rend la montée du § 25 sûre : le hub RÉUTILISE
        // l'identifiant que la sonde a donné (`ingest_probe_profiles` insère
        // avec `profile_id` tel quel). Le profil redescend donc sur la même
        // ligne, marqué comme venant du hub, au lieu d'en créer une seconde.
        //
        // S'il recevait un identifiant neuf côté hub, « Perso » apparaîtrait
        // DEUX fois à l'écran de la sonde — une fois à elle, une fois au hub —
        // et les deux seraient éditables séparément.
        let mut perso = local("perso", false);
        perso.udp_ports = vec![53];
        let out = merge(vec![perso], &[from_hub("perso", &[22, 8006])], false);

        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].from_hub, "le hub en fait autorité désormais");
        assert_eq!(out[0].tcp_ports, vec![22, 8006]);
        // L'UDP n'est pas monté : le hub ne le modélise pas, il ne peut pas en
        // faire autorité, et l'écraser détruirait un réglage irrécupérable.
        assert_eq!(out[0].udp_ports, vec![53]);
    }

    #[test]
    fn remplace_tout_ne_jette_que_ce_qui_vient_du_hub() {
        // Le drapeau dit « ce n'est pas un delta, c'est l'état du hub » : un
        // profil du hub absent de la liste a été supprimé, et sa pierre tombale
        // a pu être effacée depuis longtemps. Un profil créé ICI, lui, n'a
        // peut-être jamais été ingéré.
        let out = merge(
            vec![local("ancien-du-hub", true), local("cams", false)],
            &[from_hub("web", &[80])],
            true,
        );
        let ids: Vec<&str> = out.iter().map(|p| p.id.as_str()).collect();
        assert!(!ids.contains(&"ancien-du-hub"), "{ids:?}");
        assert!(ids.contains(&"cams"), "{ids:?}");
        assert!(ids.contains(&"web"), "{ids:?}");
    }

    #[test]
    fn la_liste_udp_locale_survit_a_une_reecriture_du_hub() {
        // ⚠️ Le hub ne modélise pas l'UDP : il ne peut pas faire autorité sur un
        // champ qu'il n'a pas. L'écraser détruirait un réglage que rien ne
        // pourrait reconstituer — ni le hub, ni la sonde.
        let mut avec_udp = local("cams", true);
        avec_udp.udp_ports = vec![5353, 1900];
        let out = merge(vec![avec_udp], &[from_hub("cams", &[554])], false);
        assert_eq!(out[0].tcp_ports, vec![554], "le hub décide du TCP");
        assert_eq!(out[0].udp_ports, vec![5353, 1900]);
    }

    #[test]
    fn un_champ_que_la_sonde_ne_modelise_pas_traverse_l_alignement() {
        // ⚠️ L'interface pose `builtin` et pourra poser autre chose demain.
        // Sans la collecte du reste, chaque alignement réécrirait la liste en
        // perdant ces champs, silencieusement et pour tout le parc.
        let mut porte_un_extra = local("cams", true);
        porte_un_extra
            .rest
            .insert("couleur".into(), serde_json::json!("rouge"));
        let out = merge(vec![porte_un_extra], &[from_hub("cams", &[554])], false);
        assert_eq!(out[0].rest.get("couleur").unwrap(), "rouge");
    }

    #[test]
    fn la_liste_se_serialise_comme_l_interface_l_ecrit() {
        // La sonde et son interface écrivent la MÊME clé : deux formes
        // divergentes donneraient deux listes à l'écran, et l'une des deux
        // disparaîtrait au premier enregistrement.
        let json = serde_json::to_value(local("cams", true)).unwrap();
        assert_eq!(json["id"], "cams");
        assert_eq!(json["tcp_ports"], serde_json::json!([22]));
        assert_eq!(json["udp_ports"], serde_json::json!([]));
        assert_eq!(json["from_hub"], true);
    }
}
