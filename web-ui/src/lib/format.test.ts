import { describe, expect, it } from 'vitest';
import { platformLabel } from './format';

describe('platformLabel', () => {
  it('rend une casse présentable, y compris macOS', () => {
    // La sonde remonte un identifiant en minuscules : bon pour une clé,
    // pas pour un écran. `macos` est le cas qu'une simple capitalisation
    // rendrait faux (« Macos »).
    expect(platformLabel('windows', '—')).toBe('Windows');
    expect(platformLabel('linux', '—')).toBe('Linux');
    expect(platformLabel('macos', '—')).toBe('macOS');
    expect(platformLabel('MACOS', '—')).toBe('macOS');
  });

  it('laisse passer une plateforme inconnue plutôt que de la masquer', () => {
    // Une sonde plus récente que le hub ne doit pas disparaître de la liste.
    expect(platformLabel('haiku', '—')).toBe('Haiku');
  });

  it('rend le texte de repli quand la plateforme est absente', () => {
    expect(platformLabel(null, 'aucune')).toBe('aucune');
    expect(platformLabel(undefined, '—')).toBe('—');
    expect(platformLabel('', '—')).toBe('—');
  });
});
