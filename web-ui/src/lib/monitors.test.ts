import { describe, expect, it } from 'vitest';
import type { MonitorEntry, MonitorState } from './api';
import { effectiveMonitors, removedMonitors } from './monitors';

const monitor = (ip: string, over: Partial<MonitorState> = {}): MonitorState => ({
  ip,
  alive: true,
  latency_ms: 3,
  min_ms: 1,
  avg_ms: 2,
  max_ms: 9,
  uptime_pct: 100,
  samples: 42,
  ...over,
});

const entry = (target: string, over: Partial<MonitorEntry> = {}): MonitorEntry => ({
  target,
  added_at: 1_000,
  removed_at: null,
  last_change_at: 1_000,
  ...over,
});

describe('effectiveMonitors', () => {
  it('garde l’annonce de la sonde quand le hub ne tient pas la liste partagée', () => {
    // Un hub antérieur à `probe_monitors` ne sert pas le champ : conclure de
    // son absence que plus rien n'est surveillé viderait l'onglet d'un parc
    // entier au premier déploiement d'interface.
    expect(effectiveMonitors(null, [monitor('10.0.0.1')])).toEqual([monitor('10.0.0.1')]);
  });

  it('ne conclut rien quand ni la sonde ni le hub n’ont de liste', () => {
    // `null` et non `[]` : « je ne sais pas » et « aucune cible » ne se disent
    // pas de la même façon à l'écran.
    expect(effectiveMonitors(null, null)).toBeNull();
    expect(effectiveMonitors([], null)).toBeNull();
  });

  it('garde active une cible annoncée que le hub ne connaît pas encore', () => {
    // ⚠️ Le piège de la bascule : une sonde antérieure au partage n'envoie
    // aucun changement, donc `probe_monitors` est VIDE alors qu'elle pingue
    // depuis des mois. Lire l'absence comme un retrait ferait disparaître
    // toutes ses cibles — le défaut même que ce chantier corrige, transposé
    // dans l'interface.
    const list = effectiveMonitors([], [monitor('10.0.0.1')]);
    expect(list?.map((m) => m.ip)).toEqual(['10.0.0.1']);
  });

  it('retire des actives une cible que le hub a retirée, même encore annoncée', () => {
    // Le retrait est un fait daté ; l'annonce de la sonde est un état qui
    // n'a pas encore reçu l'ordre. C'est le fait qui gagne, sinon la cible
    // resterait « active » jusqu'au battement suivant et le sous-onglet
    // « Retirées » dirait le contraire de la grille, au même instant.
    const list = effectiveMonitors([entry('10.0.0.9', { removed_at: 2_000 })], [monitor('10.0.0.9')]);
    expect(list).toEqual([]);
  });

  it('donne sa place à une cible ajoutée depuis le hub, sans lui inventer de mesures', () => {
    // Elle n'a pas encore de relevé : `samples` à 0 est ce qui dit à l'écran
    // de montrer l'attente plutôt qu'une disponibilité de 0 %, qui se lirait
    // comme une panne.
    const list = effectiveMonitors([entry('8.8.8.8')], []);
    expect(list).toHaveLength(1);
    expect(list?.[0].ip).toBe('8.8.8.8');
    expect(list?.[0].samples).toBe(0);
    expect(list?.[0].latency_ms).toBeNull();
  });

  it('rattache à la cible partagée les mesures que la sonde annonce', () => {
    const list = effectiveMonitors([entry('10.0.0.1')], [monitor('10.0.0.1')]);
    expect(list).toHaveLength(1);
    expect(list?.[0].samples).toBe(42);
    expect(list?.[0].uptime_pct).toBe(100);
  });

  it('ne compte jamais deux fois une cible connue des deux côtés', () => {
    const list = effectiveMonitors(
      [entry('10.0.0.1'), entry('10.0.0.2')],
      [monitor('10.0.0.2'), monitor('10.0.0.3')],
    );
    expect(list?.map((m) => m.ip)).toEqual(['10.0.0.1', '10.0.0.2', '10.0.0.3']);
  });
});

describe('removedMonitors', () => {
  it('ne rend que les cibles portant une suppression', () => {
    const list = removedMonitors([entry('10.0.0.1'), entry('10.0.0.9', { removed_at: 2_000 })]);
    expect(list.map((e) => e.target)).toEqual(['10.0.0.9']);
    expect(list[0].removed_at).toBe(2_000);
  });

  it('remonte la suppression la plus récente en tête', () => {
    // On vient à ce sous-onglet pour rattraper ce qu'on a retiré à l'instant,
    // pas pour lire une archive : le retrait d'il y a dix minutes doit être
    // au-dessus de celui d'il y a six mois.
    const list = removedMonitors([
      entry('a', { removed_at: 100 }),
      entry('b', { removed_at: 900 }),
      entry('c', { removed_at: 500 }),
    ]);
    expect(list.map((e) => e.target)).toEqual(['b', 'c', 'a']);
  });

  it('ne rend rien quand le hub ne tient pas la liste partagée', () => {
    // L'appelant distingue ce cas par lui-même : ici, pas de liste, donc rien
    // à afficher — surtout pas une affirmation d'absence de retrait.
    expect(removedMonitors(null)).toEqual([]);
  });
});
