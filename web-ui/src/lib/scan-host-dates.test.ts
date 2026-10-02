import { describe, expect, it } from 'vitest';
import type { ScanHost } from './api';
import { hostScanDates } from './scan-host-dates';

const host = (ip: string, scanned_at?: number | null): ScanHost => ({ ip, scanned_at });

describe('hostScanDates', () => {
  it('rend la date de CHAQUE machine, pas celle du lot', () => {
    // 🔴 La sonde publie toutes les machines qu'elle connaît à chaque scan de
    // ports — sans quoi le hub, qui n'affiche que le dernier scan, perdait les
    // précédentes. `started_at` date donc le LOT : une machine scannée il y a
    // une heure s'affichait comme scannée à l'instant.
    const dates = hostScanDates([host('10.0.8.1', 1_790_000_000), host('10.0.8.2', 1_790_003_600)]);
    expect(dates.get('10.0.8.1')).toBe(1_790_000_000);
    expect(dates.get('10.0.8.2')).toBe(1_790_003_600);
  });

  it('ne retient AUCUNE date pour une machine qui n’en a pas', () => {
    // ⚠️ Les lignes d'avant la v28 du schéma n'en ont pas, et une sonde
    // antérieure n'en envoie pas. L'écran doit écrire « date inconnue » :
    // inventer « maintenant » serait exactement le mensonge qu'on corrige.
    const dates = hostScanDates([host('10.0.8.1'), host('10.0.8.2', null)]);
    expect(dates.has('10.0.8.1')).toBe(false);
    expect(dates.has('10.0.8.2')).toBe(false);
  });

  it('se moque d’un zéro, qui daterait de 1970', () => {
    // Une sonde qui enverrait `0` plutôt que rien ne doit pas faire afficher le
    // 1er janvier 1970 : c'est une absence déguisée.
    expect(hostScanDates([host('10.0.8.1', 0)]).has('10.0.8.1')).toBe(false);
  });

  it('accepte un scan absent — jamais lancé, ou encore en chargement', () => {
    expect(hostScanDates(undefined).size).toBe(0);
    expect(hostScanDates(null).size).toBe(0);
  });

  it('résout tout en une passe, pour que la page ne paie pas une recherche par ligne', () => {
    // ⚠️ Un scan d'un /24 donne des centaines de machines : chercher la date
    // ligne par ligne dans le tableau des machines ferait un parcours par
    // ligne. Une table, construite une fois.
    const dates = hostScanDates(
      Array.from({ length: 200 }, (_, i) => host(`10.0.8.${i}`, 1_790_000_000 + i)),
    );
    expect(dates.size).toBe(200);
    expect(dates.get('10.0.8.199')).toBe(1_790_000_199);
  });
});
