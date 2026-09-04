import { describe, expect, it } from 'vitest';
import {
  detectPlatform,
  packageRows,
  parsePackages,
  type PackagesPayload,
} from './packages';

function payload(patch: Partial<PackagesPayload> = {}): PackagesPayload {
  return {
    reachable: true,
    error: null,
    tag: 'app-v2.1.1',
    version: '2.1.1',
    notes_url: 'https://github.com/Ops-Lorva/lanprobe/releases/tag/app-v2.1.1',
    keep_versions: 2,
    packages: [
      { platform: 'macos', version: '2.1.1', asset: 'lanprobe_v2.1.1_universal.pkg', cached: false, size: null, signed: true, publishable: true },
      { platform: 'windows', version: '2.1.1', asset: 'lanprobe_v2.1.1_x64-setup.exe', cached: false, size: null, signed: true, publishable: true },
      { platform: 'debian', version: '2.1.1', asset: 'lanprobe_v2.1.1_amd64.deb', cached: false, size: null, signed: true, publishable: true },
      { platform: 'debian-headless', version: '2.1.1', asset: 'lanprobe-server_v2.1.1_amd64.deb', cached: false, size: null, signed: false, publishable: true },
    ],
    ...patch,
  };
}

describe('paquets de la sonde', () => {
  describe('système détecté', () => {
    it('reconnaît les trois systèmes de bureau', () => {
      expect(detectPlatform('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)')).toBe('macos');
      expect(detectPlatform('Mozilla/5.0 (Windows NT 10.0; Win64; x64)')).toBe('windows');
      expect(detectPlatform('Mozilla/5.0 (X11; Linux x86_64)')).toBe('debian');
    });

    // ⚠️ Détecter aide, décider à sa place gêne. Un iPhone qui consulte le hub
    // n'installe pas de sonde : mieux vaut ne rien suggérer que suggérer faux.
    it('ne suggère rien quand il ne sait pas', () => {
      expect(detectPlatform('Mozilla/5.0 (iPhone; CPU iPhone OS 18_2)')).toBe(null);
      expect(detectPlatform('')).toBe(null);
    });

    it('ne suggère jamais le paquet sans interface', () => {
      // Le headless s'installe sur un serveur, pas sur la machine d'où l'on
      // regarde cet écran. Le proposer d'office ferait installer le mauvais.
      const rows = packageRows(payload(), 'debian');
      expect(rows.find((r) => r.pkg.platform === 'debian-headless')?.suggested).toBe(false);
    });
  });

  describe('état de chaque paquet', () => {
    // 🔴 On n'en cache aucun : le technicien peut être sur son portable et
    // installer sur une autre machine.
    it('montre les quatre plateformes, dans l’ordre du catalogue', () => {
      expect(packageRows(payload(), 'macos').map((r) => r.pkg.platform)).toEqual([
        'macos',
        'windows',
        'debian',
        'debian-headless',
      ]);
    });

    it('marque le système détecté sans écarter les autres', () => {
      const rows = packageRows(payload(), 'windows');
      expect(rows.filter((r) => r.suggested).map((r) => r.pkg.platform)).toEqual(['windows']);
    });

    it('dit ce qui est déjà en cache et ce qu’il faut récupérer', () => {
      const p = payload();
      p.packages[0] = { ...p.packages[0], cached: true, size: 42_000_000 };
      const rows = packageRows(p, null);
      expect(rows[0].state).toBe('cached');
      expect(rows[1].state).toBe('fetchable');
    });

    // 🔴 Un paquet dont la signature n'est pas publiée n'est JAMAIS servi, et
    // l'écran le dit AVANT le clic — un bouton qui échoue à tous les coups est
    // un bouton mort.
    it('nomme le paquet non signé au lieu d’offrir un bouton mort', () => {
      const rows = packageRows(payload(), null);
      expect(rows[3].state).toBe('unsigned');
    });

    // 🔴 Ce que la correction de la CI ne rattrapera PAS : les versions déjà
    // publiées n'ont pas de `.sig` pour le paquet sans interface. L'écran doit
    // donc rester vrai pour elles — pas de bouton mort, et un renvoi.
    //
    // Un état « non signé » n'existe que si le hub a LU la release : il y a
    // donc toujours une page vers laquelle renvoyer. Sans cet invariant, la
    // ligne dirait « prenez-le ailleurs » sans dire où.
    it('a toujours une page de version vers laquelle renvoyer quand il refuse', () => {
      const p = payload();
      const rows = packageRows(p, null);
      const refuses = rows.filter((r) => r.state === 'unsigned' || r.state === 'absent');
      expect(refuses.length).toBeGreaterThan(0);
      expect(p.notes_url).not.toBe(null);
      // Et aucun de ces états ne doit se confondre avec « à récupérer » : c'est
      // ce qui garantit qu'aucun bouton d'action ne leur est offert.
      expect(refuses.every((r) => r.state !== 'fetchable')).toBe(true);
    });

    it('nomme un paquet absent de la release', () => {
      const p = payload();
      p.packages[1] = { ...p.packages[1], publishable: false };
      expect(packageRows(p, null)[1].state).toBe('absent');
    });

    // ⚠️ Un hub auto-hébergé chez un client peut ne pas avoir Internet. Ce
    // qu'il a déjà se sert quand même ; le reste se DIT.
    it('sert encore ce qu’il a en cache quand GitHub est injoignable', () => {
      const p = payload({ reachable: false, error: 'dns error', tag: null, version: null });
      p.packages = p.packages.map((k) => ({ ...k, publishable: false }));
      p.packages[2] = { ...p.packages[2], cached: true, version: '2.1.0', size: 30_000_000 };
      const rows = packageRows(p, null);
      expect(rows[2].state).toBe('cached');
      expect(rows[0].state).toBe('offline');
    });
  });

  describe('lecture de la réponse', () => {
    // Un corps inattendu rend une liste vide plutôt qu'une exception : la
    // fenêtre affiche alors sa phrase, au lieu de rester en chargement.
    it('ne jette jamais sur un corps inattendu', () => {
      expect(parsePackages(null).packages).toEqual([]);
      expect(parsePackages({}).reachable).toBe(false);
      expect(parsePackages('nope').packages).toEqual([]);
    });

    it('garde les plateformes qu’il connaît et ignore les autres', () => {
      const lu = parsePackages({
        reachable: true,
        packages: [
          { platform: 'macos', cached: true, signed: true, publishable: true },
          { platform: 'solaris', cached: false, signed: true, publishable: true },
        ],
      });
      expect(lu.packages.map((p) => p.platform)).toEqual(['macos']);
    });

    // ⚠️ La version est lue ou absente, jamais inventée : un numéro en dur se
    // périme et devient un mensonge.
    it('accepte une version absente sans la remplacer', () => {
      const lu = parsePackages({ reachable: false, packages: [] });
      expect(lu.version).toBe(null);
    });
  });
});
