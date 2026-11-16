import { describe, expect, it } from 'vitest';
import { auditDetail, auditTarget, megabytes } from './audit-detail';

const T0 = Date.UTC(2026, 10, 9, 12) / 1000; // 09/11/2026
const T1 = Date.UTC(2026, 10, 16, 12) / 1000; // 16/11/2026
const SITE = '0b7c6f1e-4d3a-4c55-9a7e-2f1f2c3d4e5f';
const DEVICE = '9a1b2c3d-1111-4222-8333-444455556666';
const NOW = Date.UTC(2026, 10, 16, 15);

const t = (key: string, v: Record<string, unknown> = {}) => `${key}:${JSON.stringify(v)}`;
const opts = (names: Record<string, string> = {}) => ({ lang: 'fr', names, at: NOW, t });

describe('megabytes', () => {
  it('écrit toujours en Mo, une décimale', () => {
    expect(megabytes(412_345, 'fr')).toBe('0,4 Mo');
    expect(megabytes(40_055_000, 'fr')).toBe('38,2 Mo');
    expect(megabytes(40_055_000, 'en')).toBe('38.2 MB');
  });
  it('ne dit pas « 0 Mo » pour un petit fichier', () => {
    expect(megabytes(2_000, 'fr')).toBe('< 0,1 Mo');
  });
});

describe('auditDetail', () => {
  it('rend vide un détail vide', () => {
    expect(auditDetail(null, opts())).toBe('');
    expect(auditDetail('', opts())).toBe('');
  });

  it('laisse intact un détail sans rien à traduire', () => {
    expect(auditDetail('compte désactivé', opts())).toBe('compte désactivé');
  });

  it('remplace une période Unix par deux dates lisibles', () => {
    expect(auditDetail(`${T0}..${T1}`, opts())).toBe('09/11 → 16/11');
  });

  it('écrit l’année quand elle n’est pas la courante', () => {
    const old = Date.UTC(2025, 0, 3, 12) / 1000;
    expect(auditDetail(`${old}..${T1}`, opts())).toBe('03/01/2025 → 16/11/2026');
  });

  it('traduit une période relative isolée', () => {
    expect(auditDetail('-7d', opts())).toBe('audit.period_days:{"values":{"n":7}}');
    expect(auditDetail('-24h', opts())).toBe('audit.period_hours:{"values":{"n":24}}');
  });

  it('convertit les octets en Mo', () => {
    expect(auditDetail('40055000 octets, signature vérifiée', opts())).toBe(
      '38,2 Mo, signature vérifiée',
    );
    expect(auditDetail('le hub a servi le fichier à claire (412345 octets)', opts())).toBe(
      'le hub a servi le fichier à claire (0,4 Mo)',
    );
  });

  it('nomme un identifiant connu et retire le segment d’un identifiant inconnu', () => {
    const raw = `site ${SITE} · ${T0}..${T1} · 3 sonde(s) · depuis l'appareil ${DEVICE}`;
    expect(auditDetail(raw, opts({ [SITE]: 'Atelier Brunet' }))).toBe(
      'Atelier Brunet, 09/11 → 16/11, 3 sondes',
    );
    expect(auditDetail(raw, opts({ [DEVICE]: 'iPhone de Sophie' }))).toBe(
      "09/11 → 16/11, 3 sondes, depuis l'appareil iPhone de Sophie",
    );
  });

  it('accorde « sonde(s) »', () => {
    expect(auditDetail('1 sonde(s)', opts())).toBe('1 sonde');
    expect(auditDetail('2 archive(s) retirée(s)', opts())).toBe('2 archives retirées');
    expect(auditDetail('1 archive(s) retirée(s)', opts())).toBe('1 archive retirée');
  });

  it('remplace un horodatage Unix seul par une date', () => {
    expect(auditDetail(`expire ${T1}`, opts())).toMatch(/^expire 16\/11 \d\d:\d\d$/);
  });

  it('ne touche pas un nombre ordinaire', () => {
    expect(auditDetail('5 échecs en 2 min', opts())).toBe('5 échecs en 2 min');
  });
});

describe('auditDetail, phrase unique', () => {
  it('raccourcit un identifiant inconnu quand il n’y a pas de segment à retirer', () => {
    expect(
      auditDetail(`demandé depuis l'appareil ${DEVICE} : session web exigée`, opts()),
    ).toBe("demandé depuis l'appareil 9a1b2c3d : session web exigée");
  });
});

describe('auditTarget', () => {
  it('préfère le nom résolu par le hub', () => {
    expect(auditTarget({ target: SITE, target_name: 'Atelier Brunet' }, opts())).toEqual({
      text: 'Atelier Brunet',
      mono: false,
    });
  });
  it('nomme une archive par sa date', () => {
    expect(
      auditTarget({ target: 'lanprobe-backup-20261116-040000.tar.zst', target_name: null }, opts()),
    ).toEqual({ text: 'audit.archive_of:{"values":{"date":"16/11"}}', mono: false });
  });
  it('raccourcit un identifiant sans nom', () => {
    expect(auditTarget({ target: SITE, target_name: null }, opts())).toEqual({
      text: '0b7c6f1e',
      mono: true,
    });
  });
  it('rend le reste tel quel', () => {
    expect(auditTarget({ target: 'stagiaire', target_name: null }, opts())).toEqual({
      text: 'stagiaire',
      mono: false,
    });
    expect(auditTarget({ target: null, target_name: null }, opts())).toEqual({ text: '', mono: false });
  });
});
