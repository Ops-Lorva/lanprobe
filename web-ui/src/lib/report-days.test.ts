import { describe, expect, it } from 'vitest';
import { REPORT_DAYS_DEFAULT, REPORT_DAYS_MIN, isValidReportDays } from './report-days';

/**
 * La rétention des **fichiers** de rapport (contrat § 7 et § 23).
 *
 * 🔴 C'est le seul réglage de rétention du produit dont `0` ne veut pas dire
 * « illimité » : il est refusé. Quelqu'un le mettra par analogie avec les deux
 * autres et obtiendrait le seul réglage qui laisse s'accumuler indéfiniment
 * des données de clients sur le disque.
 */
describe('isValidReportDays', () => {
  it('accepte le défaut du hub', () => {
    expect(REPORT_DAYS_DEFAULT).toBe(7);
    expect(isValidReportDays(REPORT_DAYS_DEFAULT)).toBe(true);
  });

  it('refuse 0 — ici ce n’est PAS « illimité »', () => {
    // ⚠️ `retention_days` et `inventory_days` acceptent `0` = illimité, parce
    // qu'y toucher efface des mesures qu'on ne peut pas refaire. Un rapport,
    // lui, se régénère : le défaut sûr s'inverse. Le hub refuse `0` en 400 ;
    // le dire ici évite l'aller-retour et nomme le champ fautif.
    expect(isValidReportDays(0)).toBe(false);
    expect(isValidReportDays(-1)).toBe(false);
  });

  it('accepte le plancher, qui est celui du hub', () => {
    // Doit rester égal à `REPORT_DAYS_MIN` de `crates/lanprobe-web/src/reports.rs`.
    // Deux planchers dans deux fichiers se contredisent en silence.
    expect(REPORT_DAYS_MIN).toBe(1);
    expect(isValidReportDays(REPORT_DAYS_MIN)).toBe(true);
  });

  it('refuse ce qui n’est pas un compte de jours', () => {
    // Un champ vidé donne `NaN` : le laisser passer enverrait « NaN » au hub,
    // qui refuserait avec un message sur le type et non sur la règle.
    expect(isValidReportDays(Number.NaN)).toBe(false);
    expect(isValidReportDays(7.5)).toBe(false);
    expect(isValidReportDays(Number.POSITIVE_INFINITY)).toBe(false);
  });
});
