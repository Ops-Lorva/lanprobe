/**
 * La fenêtre de temps affichée — **une seule pour tout l'écran**.
 *
 * ⚠️ Le sélecteur n'existait que sur le tableau de bord. Les onglets
 * Surveillance, Débit, Ports et Découverte affichaient la fenêtre choisie
 * ailleurs sans le dire : on lisait « aucune cible surveillée » en croyant
 * lire le présent, alors qu'on regardait dix minutes de l'après-midi.
 *
 * D'où l'état partagé : deux sélecteurs qui divergeraient seraient pires que
 * pas de sélecteur du tout. Changer la fenêtre sur un onglet la change partout.
 *
 * Les règles vivent ici, dans un `.ts` testé, parce que le dépôt ne monte pas
 * de composants — et parce que la moitié d'entre elles (le plafond de 90
 * jours, le détour du `/metrics`) protègent InfluxDB, pas l'affichage.
 */

import { writable, get } from 'svelte/store';
import type { MetricsWindow } from './api';
import { DEFAULT_RANGE, rangeMillis } from './metrics';

/**
 * Une fenêtre : glissante (« les 6 dernières heures ») ou absolue (« du 20 au
 * 22 août »).
 *
 * `range` est une durée Flux signée (`-6h`), pas forcément l'une des cinq
 * proposées : le mode temps réel peut en imposer une autre, réglée au hub.
 */
export type WindowChoice =
  | { kind: 'relative'; range: string }
  | { kind: 'absolute'; startMs: number; stopMs: number };

/**
 * Largeur maximale d'une plage absolue, en jours.
 *
 * ⚠️ Ce n'est pas une limite d'affichage. À un ping par seconde, une plage plus
 * large met InfluxDB à genoux — et c'est l'INTERFACE qui aura l'air en panne,
 * pas la base. On refuse donc ici, avant la requête, avec un message : un
 * refus explicite se comprend, un délai d'attente non.
 */
export const MAX_RANGE_DAYS = 90;

const DAY_MS = 86_400_000;

/** Ce qui cloche dans une plage saisie. L'écran le traduit. */
export type WindowIssue = 'incomplete' | 'reversed' | 'future' | 'too_wide';

/**
 * Premier défaut d'une plage absolue, ou `null`.
 *
 * ⚠️ Une fin postérieure à « maintenant » est ACCEPTÉE : la borne de fin
 * couvre le jour entier, et « du 1er à aujourd'hui » serait autrement refusé
 * jusqu'à minuit. C'est le DÉBUT dans le futur qui décrit une plage qui
 * n'existe pas encore.
 */
export function validateAbsoluteWindow(
  startMs: number,
  stopMs: number,
  nowMs: number,
): WindowIssue | null {
  if (!Number.isFinite(startMs) || !Number.isFinite(stopMs)) return 'incomplete';
  if (stopMs <= startMs) return 'reversed';
  if (startMs > nowMs) return 'future';
  if (stopMs - startMs > MAX_RANGE_DAYS * DAY_MS) return 'too_wide';
  return null;
}

/**
 * Bornes telles que `/sla` et `/public-ips` les attendent.
 *
 * ⚠️ **Secondes**, pas millisecondes : c'est l'unité de `MetricsQuery` côté
 * hub. Envoyer des millisecondes ne lève aucune erreur — la fenêtre part en
 * l'an 57 000 et le rapport revient vide.
 */
export function toMetricsWindow(choice: WindowChoice, nowMs: number): MetricsWindow {
  if (choice.kind === 'relative') return { range: choice.range };
  return {
    start: Math.floor(choice.startMs / 1000),
    // Demander des points à venir n'a pas de sens et ferait mentir l'axe.
    stop: Math.floor(Math.min(choice.stopMs, nowMs) / 1000),
  };
}

/**
 * Une plage absolue exprimée en durée glissante, depuis son début.
 *
 * ⚠️ **N'est plus le chemin des mesures.** `probe_metrics` honore désormais
 * `start`/`stop` (crates/lanprobe-web/src/web.rs), et l'écran passe par
 * `toMetricsWindow` comme `/sla` et `/public-ips`. Le sur-tirage que cette
 * fonction organisait — tout demander depuis le début de la plage, puis couper
 * la fin côté navigateur — faisait lire quatre-vingts jours de points pour en
 * afficher un, et surtout : le hub agrège sur la fenêtre DEMANDÉE. Réclamer
 * 87 jours pour en afficher 10 minutes ferait moyenner par trois heures ce qui
 * tient très bien à la seconde.
 *
 * Conservée pour qui a besoin d'une durée relative à partir d'une plage.
 */
export function metricsRange(choice: WindowChoice, nowMs: number): string {
  if (choice.kind === 'relative') return choice.range;
  const seconds = Math.max(1, Math.ceil((nowMs - choice.startMs) / 1000));
  return `-${seconds}s`;
}

/** Ne garde que les points de la plage. Sans effet sur une fenêtre glissante. */
export function clipToWindow<T extends { t: number }>(
  points: T[],
  choice: WindowChoice,
): T[] {
  if (choice.kind === 'relative') return points;
  return points.filter((p) => p.t >= choice.startMs && p.t <= choice.stopMs);
}

/**
 * Bornes de l'axe. La fenêtre DEMANDÉE borne le graphe, même si les mesures
 * n'en couvrent qu'un coin : « 6 h » doit montrer six heures, sinon trente
 * minutes de relevés occupent toute la largeur et on croit lire six heures.
 */
export function windowDomain(choice: WindowChoice, loadedAtMs: number): { from: number; to: number } {
  if (choice.kind === 'absolute') return { from: choice.startMs, to: choice.stopMs };
  return { from: loadedAtMs - rangeMillis(choice.range), to: loadedAtMs };
}

/** Durée Flux d'un nombre de minutes (`10` → `-10m`). */
export function relativeMinutes(minutes: number): string {
  return `-${Math.max(1, Math.round(minutes))}m`;
}

/**
 * Lit un champ `<input type="date">` en heure LOCALE.
 *
 * ⚠️ `new Date('2026-08-31')` est interprété en UTC par la spécification, ce
 * qui décale la borne d'un fuseau entier — jusqu'à une journée de mesures qui
 * change de côté. On passe donc par le constructeur à composantes, qui est
 * local, comme le fait déjà l'export SLA du parc.
 *
 * La borne `stop` couvre le jour ENTIER : « du 1er au 31 » s'arrêterait sinon
 * à minuit le 31 au matin, et le dernier jour manquerait au rapport sans que
 * personne ne s'en aperçoive.
 */
export function parseLocalDate(value: string, bound: 'start' | 'stop'): number {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value.trim());
  if (!m) return Number.NaN;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  const at =
    bound === 'start'
      ? new Date(y, mo - 1, d, 0, 0, 0, 0)
      : new Date(y, mo - 1, d, 23, 59, 59, 0);
  const ms = at.getTime();
  // Un 31 février donne le 3 mars : la saisie ne décrit pas le jour demandé.
  return at.getDate() === d && at.getMonth() === mo - 1 ? ms : Number.NaN;
}

/** L'état que la bascule du temps réel manipule. */
export interface WindowState {
  choice: WindowChoice;
  /**
   * Fenêtre à rétablir à l'extinction du mode temps réel. `null` quand il n'y
   * a rien à rétablir — mode éteint, ou choix explicite fait pendant le mode.
   */
  restore: WindowChoice | null;
}

/**
 * Bascule automatique du mode temps réel.
 *
 * ⚠️ **On propose, on n'impose pas.** Les autres fenêtres restent cliquables
 * pendant le mode : `restore` est vidé au premier clic (`pickWindow`), et
 * l'extinction ne défait alors plus rien. Sans cette mémoire, l'écran
 * rebasculerait sur 10 min à chaque tour d'horloge — toutes les 3 s sur une
 * fiche ouverte — et le clic de l'utilisateur serait annulé sous ses yeux.
 */
export function applyRealtime(
  state: WindowState,
  mode: { on: boolean; windowMin: number },
): WindowState {
  if (mode.on) {
    // Déjà basculé : ne rien refaire, sinon on écrase un choix explicite.
    if (state.restore !== null) return state;
    return {
      choice: { kind: 'relative', range: relativeMinutes(mode.windowMin) },
      restore: state.choice,
    };
  }
  if (state.restore === null) return state;
  return { choice: state.restore, restore: null };
}

// ── État partagé ────────────────────────────────────────────────────────────

/**
 * La fenêtre, tenue en UN seul endroit.
 *
 * ⚠️ Un état par onglet aurait été plus simple à écrire et pire à utiliser :
 * on règle « 24 h » sur le tableau de bord, on passe sur Surveillance, et la
 * page montre dix minutes sans rien dire. Deux sélecteurs qui divergent sont
 * pires que pas de sélecteur.
 */
function createWindowStore() {
  const store = writable<WindowState>({
    choice: { kind: 'relative', range: DEFAULT_RANGE },
    restore: null,
  });

  return {
    subscribe: store.subscribe,

    /**
     * Choix explicite de l'utilisateur.
     *
     * ⚠️ `restore` est vidé : pendant le mode temps réel, un clic sur une autre
     * fenêtre doit TENIR. Le garder ferait revenir l'écran à la fenêtre d'avant
     * le mode à son extinction, c'est-à-dire annuler le choix sans prévenir.
     * On propose la bonne fenêtre, on ne l'impose pas.
     */
    pick(choice: WindowChoice) {
      store.set({ choice, restore: null });
    },

    /** Bascule du temps réel. Idempotente : appelable à chaque tour d'horloge. */
    syncRealtime(mode: { on: boolean; windowMin: number }) {
      const current = get(store);
      const next = applyRealtime(current, mode);
      if (next !== current) store.set(next);
    },
  };
}

export const timeWindow = createWindowStore();
