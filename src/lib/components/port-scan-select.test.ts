import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

/**
 * QA 16/11/2026 (branche fix/audit-actions-traduites) — anomalie constatée à
 * l'écran, corrigée : sur une sonde rattachée à un hub, les profils
 * de base sont masqués (`visibleProfiles`) mais le profil actif par défaut reste
 * `builtin:common`. Les deux sélecteurs de profil de PortScan.svelte lisent cet
 * identifiant BRUT (`activeId`) au lieu du profil résolu (`$currentProfile`,
 * qui existe précisément pour ce cas) : aucune `<option>` ne correspond, la
 * case se dessine vide, et le scan part pourtant avec un profil de repli.
 */
const src = readFileSync(join(__dirname, 'PortScan.svelte'), 'utf8');

describe('sélecteurs de profil de scan de ports', () => {
  it("le sélecteur du haut affiche le profil résolu, pas l'identifiant brut", () => {
    expect(src).not.toMatch(/<select[^>]*value=\{activeId\}/);
  });

  it('« Rescanner avec » affiche un profil visible, pas activeId brut', () => {
    expect(src).not.toMatch(/value=\{current\.profileId \?\? activeId\}/);
  });
});
