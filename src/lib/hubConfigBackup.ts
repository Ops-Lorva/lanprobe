/**
 * Sauvegarde des profils locaux auprès du hub (contrat § 16).
 *
 * ⚠️ Le hub ne s'en sert pas et ne les affiche pas. Les profils réseau servent
 * en local, devant la machine, pour changer de réseau en trois clics — les
 * montrer dans le hub n'apprendrait rien. Il est un **concentrateur** : il les
 * garde, ils partent dans ses sauvegardes, et une machine réinstallée les
 * retrouve à son ré-enrôlement. C'est tout le service rendu, et c'est celui
 * qu'on cherche.
 */

import { invoke } from '@tauri-apps/api/core';
import { getConfigStore } from './stores/configStore';

/** Clés du magasin local qui méritent d'être sauvegardées. */
const BACKED_UP = ['profiles', 'portscan_profiles', 'scheduler', 'monitoring_hosts'] as const;

/**
 * Dépose l'état courant au hub.
 *
 * ⚠️ Groupé et différé : chaque édition de profil déclencherait sinon une
 * requête, et renommer un profil caractère par caractère en produirait une
 * douzaine. Une seconde de calme suffit.
 */
let timer: ReturnType<typeof setTimeout> | null = null;

export function scheduleBackup(): void {
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => {
    timer = null;
    void pushNow();
  }, 1000);
}

export async function pushNow(): Promise<void> {
  try {
    const store = await getConfigStore();
    const config: Record<string, unknown> = {};
    for (const key of BACKED_UP) {
      const value = await store.get(key);
      if (value !== undefined && value !== null) config[key] = value;
    }
    if (Object.keys(config).length === 0) return;
    await invoke('cmd_hub_push_config', { config });
  } catch {
    // ⚠️ Silencieux, volontairement. Les profils restent sur la machine ; une
    // sauvegarde manquée ne doit ni interrompre une saisie, ni afficher une
    // erreur pour une fonction dont l'utilisateur n'a rien demandé.
  }
}

/**
 * Restaure ce que le hub gardait, après un ré-enrôlement.
 *
 * ⚠️ N'écrase que les clés ABSENTES en local. Une machine réinstallée est vide
 * et récupère tout ; une machine qui vient d'être ré-enrôlée après une
 * révocation a ses profils, et se les faire remplacer par une photo plus
 * ancienne serait une perte.
 *
 * Rend le nombre de clés effectivement restaurées.
 */
export async function restoreFromHub(): Promise<number> {
  let restored: Record<string, unknown> | null = null;
  try {
    restored = await invoke<Record<string, unknown> | null>('cmd_hub_take_restored_config');
  } catch {
    return 0;
  }
  if (!restored) return 0;

  const store = await getConfigStore();
  let count = 0;
  for (const [key, value] of Object.entries(restored)) {
    const existing = await store.get(key);
    const empty = existing === undefined || existing === null || (Array.isArray(existing) && existing.length === 0);
    if (!empty) continue;
    await store.set(key, value);
    count += 1;
  }
  if (count > 0) await store.save();
  return count;
}
