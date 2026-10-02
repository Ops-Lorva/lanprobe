import { derived, writable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { scheduleBackup } from '../hubConfigBackup';
import { getConfigStore } from './configStore';
import {
  activeProfile,
  visibleProfiles,
} from '../../../web-ui/src/lib/portscan-profile-visibility';

export interface PortScanProfile {
  id: string;
  name: string;
  tcp_ports: number[];
  udp_ports: number[];
  builtin?: boolean;
  /**
   * Posé par le backend quand le profil vient du HUB (contrat § 25).
   *
   * ⚠️ L'écran doit le dire : le hub fait autorité, et une modification locale
   * sera réécrite au prochain battement. Laisser croire à une édition qui
   * tiendra serait un mensonge d'interface.
   *
   * ⚠️ Ces profils se persistent comme les autres — ils ne sont pas `builtin`.
   * Les filtrer à l'écriture les ferait disparaître du fichier, et la sonde
   * les redemanderait au hub à chaque démarrage.
   */
  from_hub?: boolean;
}

const STORE_KEY = 'portscan_profiles';
const ACTIVE_KEY = 'portscan_active_profile';

// Profils fournis par défaut. Non persistés (recalculés à chaque init) —
// on ne veut pas qu'une mise à jour de LanProbe laisse en base l'ancienne
// liste de ports d'un preset.
export const BUILTIN_PROFILES: PortScanProfile[] = [
  {
    id: 'builtin:common',
    name: 'Common',
    tcp_ports: [21, 22, 23, 25, 53, 80, 110, 143, 443, 445, 3306, 3389, 5432, 5900, 8080, 8443],
    udp_ports: [53, 123, 137, 161, 1900, 5353],
    builtin: true,
  },
  {
    id: 'builtin:web',
    name: 'Web',
    tcp_ports: [80, 443, 8000, 8008, 8080, 8081, 8088, 8181, 8443, 8888, 3000, 5000, 9000],
    udp_ports: [],
    builtin: true,
  },
  {
    id: 'builtin:db',
    name: 'Databases',
    tcp_ports: [1433, 1521, 3306, 5432, 5984, 6379, 7000, 9042, 9200, 9300, 11211, 27017, 50000],
    udp_ports: [1434],
    builtin: true,
  },
  {
    id: 'builtin:remote',
    name: 'Remote access',
    tcp_ports: [22, 23, 2222, 3389, 5900, 5901, 5902, 5938, 6000],
    udp_ports: [],
    builtin: true,
  },
  {
    id: 'builtin:full',
    name: 'Full (extended)',
    tcp_ports: [
      21, 22, 23, 25, 53, 80, 110, 111, 135, 139, 143, 389, 443, 445, 465, 514, 587,
      631, 636, 873, 993, 995, 1080, 1194, 1433, 1521, 2049, 2222, 2375, 2376,
      3000, 3128, 3306, 3389, 5000, 5060, 5222, 5432, 5672, 5900, 5984, 6379, 6443,
      7000, 8000, 8008, 8080, 8086, 8088, 8181, 8443, 8883, 9000, 9092, 9200, 9300,
      11211, 27017, 50000,
    ],
    udp_ports: [53, 67, 68, 69, 123, 137, 161, 500, 514, 1900, 4500, 5353],
    builtin: true,
  },
];

function createPortScanProfilesStore() {
  const { subscribe, set, update } = writable<PortScanProfile[]>(BUILTIN_PROFILES);
  const active = writable<string>('builtin:common');

  /**
   * La sonde est-elle rattachée à un hub ?
   *
   * 🔴 C'est ce qui décide si ses profils de BASE s'affichent encore
   * (contrat § 25, décision du 02/10) : le hub sème les mêmes et fait autorité,
   * alors les montrer tous les deux donnait trois paires de même nom aux
   * contenus différents — `Common` 16T/6U en local contre 0/0 au hub.
   *
   * ⚠️ Repli sur « non rattachée » si la question échoue : on affiche TOUT
   * plutôt que rien. Un écran sans aucun profil serait pire que des doublons.
   */
  const enrolled = writable<boolean>(false);

  async function refreshEnrolment() {
    try {
      const hub = await invoke<{ enrolled?: boolean } | null>('cmd_hub_status');
      enrolled.set(hub?.enrolled === true);
    } catch {
      enrolled.set(false);
    }
  }

  async function init() {
    const store = await getConfigStore();
    const saved = await store.get<PortScanProfile[]>(STORE_KEY);
    const custom = (saved ?? []).filter(p => !p.builtin && !p.id.startsWith('builtin:'));
    set([...BUILTIN_PROFILES, ...custom]);
    const savedActive = await store.get<string>(ACTIVE_KEY);
    if (savedActive) active.set(savedActive);
    // ⚠️ Relu à chaque `init()`, donc à chaque `config:update` : rattacher la
    // sonde à un hub doit faire disparaître les profils de base SANS recharger
    // la fenêtre, comme l'arrivée d'un profil du hub les fait apparaître.
    await refreshEnrolment();
  }

  async function persist(profiles: PortScanProfile[]) {
    const store = await getConfigStore();
    const custom = profiles.filter(p => !p.builtin && !p.id.startsWith('builtin:'));
    await store.set(STORE_KEY, custom);
    await store.save();
    scheduleBackup();
  }

  async function persistActive(id: string) {
    const store = await getConfigStore();
    await store.set(ACTIVE_KEY, id);
    await store.save();
  }

  /**
   * Ce que les sélecteurs affichent — profils de base exclus dès que la sonde
   * est rattachée à un hub. La liste complète reste derrière `subscribe` :
   * c'est elle qu'on persiste, qu'on sauvegarde au hub et qu'on restaure.
   *
   * ⚠️ Rien n'est supprimé : les profils de base resservent tels quels le jour
   * où la sonde est désenrôlée.
   */
  const visible = derived([{ subscribe }, enrolled], ([all, isEnrolled]) =>
    visibleProfiles(all as PortScanProfile[], isEnrolled as boolean),
  );

  /**
   * Le profil actif **parmi ceux qui s'affichent**.
   *
   * 🔴 Une sonde qu'on vient d'enrôler garde `builtin:common` comme profil
   * actif, devenu invisible : sans ce repli, le sélecteur n'afficherait aucune
   * sélection et le scan partirait avec une liste de ports que l'écran ne
   * montre plus. `undefined` quand il ne reste aucun profil — l'appelant le
   * traduit en « la sonde garde sa propre liste », jamais en « aucun port ».
   */
  const current = derived([visible, active], ([liste, id]) =>
    activeProfile(liste as PortScanProfile[], id as string),
  );

  return {
    subscribe,
    init,
    enrolled: { subscribe: enrolled.subscribe },
    visible: { subscribe: visible.subscribe },
    current: { subscribe: current.subscribe },
    active: { subscribe: active.subscribe },
    setActive: (id: string) => { active.set(id); persistActive(id); },
    add: (p: PortScanProfile) => update(profiles => {
      const next = [...profiles, p];
      persist(next);
      return next;
    }),
    edit: (p: PortScanProfile) => update(profiles => {
      const next = profiles.map(x => x.id === p.id ? p : x);
      persist(next);
      return next;
    }),
    remove: (id: string) => update(profiles => {
      const next = profiles.filter(x => x.id !== id);
      persist(next);
      return next;
    }),
  };
}

export const portscanProfiles = createPortScanProfilesStore();
