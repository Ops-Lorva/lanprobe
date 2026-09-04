import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from './api';
import {
  REPORT_POLL_MS,
  ReportAborted,
  ReportIntegrityError,
  exactBytes,
  fetchReportFile,
  hubLacksReports,
  integrityFault,
  parseReports,
  pollReport,
  purgedAt,
  reportErrorMessage,
  reportRows,
  reportsApi,
  type HubReport,
} from './reports';

/**
 * Les rapports produits par le HUB (contrat § 23), côté interface.
 *
 * 🔴 Ce qui se teste ici tient à une seule phrase du contrat : « un
 * téléchargement à moitié réussi se solde par un échec visible, jamais par un
 * fichier disponible ». Un classeur tronqué s'ouvre très bien, avec moins de
 * lignes, et part chez un client. C'est la valeur plausible et fausse la plus
 * chère que ce produit sache fabriquer, et la seule chose qui l'arrête est le
 * contrôle écrit ici.
 *
 * Le reste — l'historique, l'état, le repli — existe pour que l'écran DISE où
 * il en est. Un bouton muet pendant trente secondes se lit comme une panne, et
 * un rapport purgé qui échoue en silence se lit comme un hub cassé.
 */

function report(over: Partial<HubReport> = {}): HubReport {
  return {
    report_id: 'r1',
    site_id: 's1',
    site_name: 'Durand',
    requested_by: 'benjamin',
    device_id: null,
    probe_ids: ['p1'],
    locale: 'fr',
    range_start: 1_700_000_000,
    range_stop: 1_700_600_000,
    state: 'ready',
    step: null,
    error: null,
    file_name: 'lanprobe-sla-durand.xlsx',
    file_size: 248_013,
    file_sha256: 'ab'.repeat(32),
    requested_at: 1_700_600_100,
    ready_at: 1_700_600_120,
    expires_at: 1_701_204_920,
    purged_at: null,
    downloads: 0,
    ...over,
  };
}

describe('parseReports', () => {
  it('lit l’enveloppe `reports` servie par le hub', () => {
    // `list_site_reports` rend `{ "reports": [...] }` — vérifié dans
    // `crates/lanprobe-web/src/reports.rs`, pas supposé.
    expect(parseReports({ reports: [report({ report_id: 'a' })] }).map((r) => r.report_id)).toEqual([
      'a',
    ]);
  });

  it('lit aussi une liste nue', () => {
    expect(parseReports([report({ report_id: 'b' })]).map((r) => r.report_id)).toEqual(['b']);
  });

  it('rend une liste vide plutôt que de jeter sur une réponse inattendue', () => {
    // Une exception ici laisserait l'historique en chargement perpétuel, sans
    // rien à lire — exactement ce qu'on interdit face à un hub plus ancien.
    expect(parseReports(null)).toEqual([]);
    expect(parseReports(undefined)).toEqual([]);
    expect(parseReports({ error: 'route inconnue' })).toEqual([]);
    expect(parseReports('rapports non implémentés')).toEqual([]);
  });
});

describe('reportRows', () => {
  it('met le plus récemment demandé en tête', () => {
    const rows = reportRows([
      report({ report_id: 'vieux', requested_at: 1_700_000_000 }),
      report({ report_id: 'frais', requested_at: 1_700_600_000 }),
    ]);
    expect(rows.map((r) => r.report.report_id)).toEqual(['frais', 'vieux']);
  });

  it('ne rend téléchargeable que ce qui a un fichier', () => {
    expect(reportRows([report({ state: 'ready' })])[0].downloadable).toBe(true);
    expect(reportRows([report({ state: 'preparing' })])[0].downloadable).toBe(false);
    expect(reportRows([report({ state: 'failed' })])[0].downloadable).toBe(false);
    expect(reportRows([report({ state: 'purged' })])[0].downloadable).toBe(false);
  });

  it('distingue « purgé » de « échoué » — la ligne reste, le fichier non', () => {
    // 🔴 Un rapport purgé n'est pas un rapport raté : il a existé, quelqu'un
    // l'a eu, et l'écran doit proposer « Redemander » plutôt qu'afficher une
    // erreur qui ferait douter du hub.
    const purge = reportRows([report({ state: 'purged', purged_at: 1_701_204_920 })])[0];
    expect(purge.purged).toBe(true);
    expect(purge.failed).toBe(false);
    expect(purge.report.purged_at).toBe(1_701_204_920);

    const raté = reportRows([report({ state: 'failed', error: 'Influx injoignable' })])[0];
    expect(raté.failed).toBe(true);
    expect(raté.purged).toBe(false);
  });

  it('traite un état inconnu comme non téléchargeable', () => {
    // Un hub plus récent peut nommer un état que cet écran ne connaît pas.
    // Proposer le téléchargement « au cas où » offrirait un bouton qui échoue.
    const rows = reportRows([report({ state: 'quelque_chose_de_neuf' })]);
    expect(rows[0].downloadable).toBe(false);
    expect(rows[0].purged).toBe(false);
  });
});

describe('hubLacksReports', () => {
  it('reconnaît un hub qui n’a pas encore la route', () => {
    // `501` : la route existe mais n'est pas écrite.
    expect(hubLacksReports(new ApiError(501, 'non implémenté'))).toBe(true);
    // `404` NU : axum ne trouve pas la route et ne rend aucun corps.
    expect(hubLacksReports(new ApiError(404, 'HTTP 404', null))).toBe(true);
  });

  it('ne confond JAMAIS un site hors portée avec une route absente', () => {
    // 🔴 `site_scope_guard` rend `404 { "error": "site inconnu" }` pour un site
    // qui n'existe pas OU qui appartient à un autre client. Le prendre pour un
    // hub trop ancien ferait produire le classeur par le navigateur — donc
    // livrerait par une autre porte les données que la portée refuse.
    expect(hubLacksReports(new ApiError(404, 'site inconnu', { error: 'site inconnu' }))).toBe(
      false,
    );
    expect(hubLacksReports(new ApiError(404, 'sonde inconnue', { error: 'sonde inconnue' }))).toBe(
      false,
    );
  });

  it('ne prend pas un refus ordinaire pour une absence de route', () => {
    expect(hubLacksReports(new ApiError(403, 'rôle insuffisant'))).toBe(false);
    expect(hubLacksReports(new ApiError(400, 'aucune sonde à mettre dans ce rapport'))).toBe(false);
    expect(hubLacksReports(new ApiError(0, 'network'))).toBe(false);
    expect(hubLacksReports(new Error('autre chose'))).toBe(false);
    expect(hubLacksReports(null)).toBe(false);
  });
});

/** Empreinte hexadécimale de référence, calculée par le même moteur que le hub. */
async function empreinteDe(bytes: Uint8Array): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', bytes as unknown as ArrayBuffer);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
}

describe('integrityFault', () => {
  const OCTETS = new Uint8Array([80, 75, 3, 4, 20, 0, 6, 0]);

  it('laisse passer des octets complets et conformes', async () => {
    expect(
      await integrityFault({ length: OCTETS.length, sha256: await empreinteDe(OCTETS) }, OCTETS),
    ).toBeNull();
  });

  it('refuse un corps tronqué, et dit LES DEUX nombres', async () => {
    // 🔴 « erreur réseau » ne permet pas de décider s'il faut réessayer ou
    // appeler. « attendu 248 013 o · reçu 91 220 o », si.
    const faute = await integrityFault({ length: 248_013, sha256: null }, OCTETS);
    expect(faute).toEqual({ kind: 'length', expected: 248_013, received: OCTETS.length });
  });

  it('refuse un corps de la bonne taille dont l’empreinte diffère', async () => {
    // Le seul contrôle qui attrape une corruption silencieuse du volume :
    // un classeur de la bonne longueur, aux octets changés, s'ouvre très bien.
    const faute = await integrityFault(
      { length: OCTETS.length, sha256: 'cd'.repeat(32) },
      OCTETS,
    );
    expect(faute?.kind).toBe('digest');
  });

  it('refuse ce qu’il ne peut pas vérifier du tout', async () => {
    // Ni longueur annoncée ni empreinte : il n'existe alors aucun moyen de
    // savoir si le fichier est entier. Un échec nommé vaut mieux qu'un classeur
    // dont personne ne peut rien dire.
    expect(await integrityFault({ length: null, sha256: null }, OCTETS)).toEqual({
      kind: 'unverifiable',
    });
  });

  it('se contente de la longueur quand le navigateur n’a pas de crypto', async () => {
    // ⚠️ `crypto.subtle` n'existe QUE dans un contexte sécurisé. Un hub
    // auto-hébergé servi en clair sur une IP de LAN est le cas COURANT : y
    // refuser tout téléchargement rendrait la fonctionnalité inutilisable là
    // où elle sert le plus. La longueur reste vérifiée, elle.
    vi.stubGlobal('crypto', {});
    expect(
      await integrityFault({ length: OCTETS.length, sha256: 'cd'.repeat(32) }, OCTETS),
    ).toBeNull();
    expect(await integrityFault({ length: 3, sha256: 'cd'.repeat(32) }, OCTETS)).toMatchObject({
      kind: 'length',
    });
  });
});

describe('exactBytes', () => {
  it('rend les octets EXACTS, jamais un arrondi', () => {
    // ⚠️ `humanBytes` dirait « 242,2 Ko » des deux côtés d'un fichier amputé de
    // trois cents octets : le message censé trancher n'apprendrait rien.
    expect(exactBytes(248_013, 'en')).toBe('248,013');
    expect(exactBytes(248_013, 'fr').replace(/ | /g, ' ')).toBe('248 013');
  });
});

describe('purgedAt', () => {
  it('lit la date de purge du 410, pour que l’écran DISE laquelle', () => {
    // ⚠️ `410`, pas `404` : la ligne existe, l'app vient de l'afficher. Sans
    // cette date, l'écran ne peut dire que « erreur », et un rapport purgé se
    // lit comme un hub cassé.
    const e = new ApiError(410, 'le fichier de ce rapport a été purgé', {
      error: 'le fichier de ce rapport a été purgé',
      state: 'purged',
      purged_at: 1_701_204_920,
    });
    expect(purgedAt(e)).toBe(1_701_204_920);
  });

  it('rend `null` sur tout le reste', () => {
    expect(purgedAt(new ApiError(500, 'boum'))).toBeNull();
    expect(purgedAt(new ApiError(410, 'purgé', { state: 'purged' }))).toBeNull();
    expect(purgedAt(null)).toBeNull();
  });
});

/** Dernier appel `fetch` observé, avec une réponse JSON. */
function captureFetch(body: unknown, status = 200) {
  const spy = vi.fn(
    async (_url: string, _init?: RequestInit) =>
      new Response(JSON.stringify(body), {
        status,
        headers: { 'Content-Type': 'application/json' },
      }),
  );
  vi.stubGlobal('fetch', spy);
  return spy;
}

afterEach(() => vi.unstubAllGlobals());

describe('reportsApi', () => {
  it('demande le rapport sur la route DU SITE, la fenêtre résolue', async () => {
    // ⚠️ Une fenêtre glissante figée en bornes exactes : « les 7 derniers
    // jours » relus à la génération donneraient un autre chiffre que celui vu
    // à l'écran.
    const spy = captureFetch({ report_id: 'r9', state: 'preparing' }, 202);
    await reportsApi.requestReport('site/1', {
      probe_ids: ['p1', 'p2'],
      window: { start: 1_700_000_000, stop: 1_700_600_000 },
    });
    const [url, init] = spy.mock.calls[0];
    if (!init) throw new Error('aucune option de requête');
    expect(url).toBe('/api/sites/site%2F1/reports');
    expect(init.method).toBe('POST');
    // 🔴 **Plus de `locale` dans la demande.** La langue du classeur est celle
    // du SITE (`sites.report_locale`), lue par le hub : le document part chez
    // un client, et deux opérateurs ne doivent pas lui envoyer deux langues.
    // Continuer à l'envoyer ferait croire au suivant qu'elle décide encore.
    expect(JSON.parse(String(init.body))).toEqual({
      probe_ids: ['p1', 'p2'],
      start: 1_700_000_000,
      stop: 1_700_600_000,
    });
  });

  it('n’envoie JAMAIS une liste de sondes vide', async () => {
    // 🔴 `probe_ids` absent = toutes les sondes du site ; une liste vide est
    // refusée par le hub. Les confondre produirait le classeur du site entier
    // là où l'écran affichait zéro case cochée.
    const spy = captureFetch({ report_id: 'r9', state: 'preparing' }, 202);
    await reportsApi.requestReport('s1', { window: { range: '-7d' } });
    const [, init] = spy.mock.calls[0];
    const envoyé = JSON.parse(String(init?.body));
    expect('probe_ids' in envoyé).toBe(false);
    expect(envoyé.range).toBe('-7d');
  });

  it('laisse partir une liste VIDE fournie explicitement, pour que le hub la refuse', async () => {
    // 🔴 Vide n'est pas « toutes ». La transformer en « absent » ici produirait
    // le classeur du site entier là où l'écran affichait zéro case cochée — et
    // le classeur part chez un client. Le hub, lui, refuse en `400` avec une
    // phrase qui nomme la règle.
    const spy = captureFetch({ error: 'aucune sonde à mettre dans ce rapport' }, 400);
    await reportsApi
      .requestReport('s1', { probe_ids: [], window: { range: '-7d' } })
      .catch(() => {});
    const [, init] = spy.mock.calls[0];
    expect(JSON.parse(String(init?.body)).probe_ids).toEqual([]);
  });

  it('suit un rapport par son identifiant, pas par son site', async () => {
    const spy = captureFetch(report());
    await reportsApi.trackReport('r/1');
    expect(spy.mock.calls[0][0]).toBe('/api/reports/r%2F1');
  });

  it('liste l’historique sur la route du site', async () => {
    // ⚠️ Route DE SITE : la garde de portée s'applique avant le handler. Un
    // `GET /api/reports?site=` aurait laissé ce contrôle au handler.
    const spy = captureFetch({ reports: [] });
    await reportsApi.listSiteReports('s1');
    expect(spy.mock.calls[0][0]).toBe('/api/sites/s1/reports');
  });

  it('laisse remonter le refus du hub, message compris', async () => {
    captureFetch({ error: 'aucune sonde à mettre dans ce rapport' }, 400);
    await expect(
      reportsApi.requestReport('s1', { probe_ids: ['p1'], window: { range: '-7d' } }),
    ).rejects.toMatchObject({ status: 400, message: 'aucune sonde à mettre dans ce rapport' });
  });
});

describe('pollReport', () => {
  it('interroge jusqu’à un état terminal et rend la ligne finale', async () => {
    const étapes: HubReport[] = [
      report({ state: 'preparing', step: 'sonde 1 sur 3 — Lyon' }),
      report({ state: 'preparing', step: 'sonde 2 sur 3 — Paris' }),
      report({ state: 'ready' }),
    ];
    const vus: (string | null)[] = [];
    const final = await pollReport('r1', {
      track: async () => étapes.shift() ?? report({ state: 'ready' }),
      sleep: async () => {},
      onStep: (r) => vus.push(r.step),
    });
    expect(final.state).toBe('ready');
    // L'écran doit dire OÙ IL EN EST : un bouton muet trente secondes se lit
    // comme une panne.
    expect(vus).toEqual(['sonde 1 sur 3 — Lyon', 'sonde 2 sur 3 — Paris']);
  });

  it('rend aussi un échec, sans jeter : la cause est nommée dans la ligne', async () => {
    // ⚠️ « échec » ne dit pas s'il faut réessayer ou appeler ; `error` le dit.
    const final = await pollReport('r1', {
      track: async () => report({ state: 'failed', error: 'le hub a redémarré pendant la préparation' }),
      sleep: async () => {},
    });
    expect(final.state).toBe('failed');
    expect(final.error).toBe('le hub a redémarré pendant la préparation');
  });

  it('s’arrête quand l’écran se ferme, sans rappeler le hub', async () => {
    const abandon = { aborted: false };
    let appels = 0;
    const promesse = pollReport('r1', {
      track: async () => {
        appels++;
        abandon.aborted = true;
        return report({ state: 'preparing' });
      },
      sleep: async () => {},
      signal: abandon,
    });
    await expect(promesse).rejects.toMatchObject({ name: 'ReportAborted' });
    expect(appels).toBe(1);
  });

  it('attend entre deux interrogations — le hub ne prépare qu’un rapport à la fois', async () => {
    const attentes: number[] = [];
    let reste = 2;
    await pollReport('r1', {
      track: async () => report({ state: reste-- > 0 ? 'preparing' : 'ready' }),
      sleep: async (ms) => {
        attentes.push(ms);
      },
    });
    expect(attentes.every((ms) => ms === REPORT_POLL_MS)).toBe(true);
    expect(attentes.length).toBeGreaterThan(0);
  });
});

describe('fetchReportFile', () => {
  const CLASSEUR = new Uint8Array([80, 75, 3, 4, 20, 0, 6, 0, 8, 0]);

  function serveFile(bytes: Uint8Array, headers: Record<string, string>) {
    const spy = vi.fn(
      async (_url: string, _init?: RequestInit) =>
        new Response(bytes as unknown as BodyInit, {
          status: 200,
          headers: {
            'Content-Type':
              'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
            ...headers,
          },
        }),
    );
    vi.stubGlobal('fetch', spy);
    return spy;
  }

  it('rend les octets et le nom quand tout concorde', async () => {
    const empreinte = await empreinteDe(CLASSEUR);
    serveFile(CLASSEUR, {
      'Content-Length': String(CLASSEUR.length),
      'X-LanProbe-Sha256': empreinte,
      'Content-Disposition': 'attachment; filename="lanprobe-sla-durand.xlsx"',
    });
    const fichier = await fetchReportFile('r1');
    expect(fichier.bytes.length).toBe(CLASSEUR.length);
    expect(fichier.fileName).toBe('lanprobe-sla-durand.xlsx');
  });

  it('ÉCHOUE sur un corps tronqué, plutôt que de rendre un classeur amputé', async () => {
    // 🔴 La règle du contrat § 23, en une ligne de test : un téléchargement à
    // moitié réussi se solde par un échec visible, jamais par un fichier.
    serveFile(CLASSEUR, {
      'Content-Length': '248013',
      'X-LanProbe-Sha256': await empreinteDe(CLASSEUR),
    });
    await expect(fetchReportFile('r1')).rejects.toMatchObject({
      fault: { kind: 'length', expected: 248_013, received: CLASSEUR.length },
    });
  });

  it('ÉCHOUE quand l’empreinte ne correspond pas aux octets reçus', async () => {
    serveFile(CLASSEUR, {
      'Content-Length': String(CLASSEUR.length),
      'X-LanProbe-Sha256': 'cd'.repeat(32),
    });
    await expect(fetchReportFile('r1')).rejects.toMatchObject({ fault: { kind: 'digest' } });
  });

  it('traduit le 410 du fichier purgé en `ApiError`, date comprise', async () => {
    // L'écran affiche « fichier purgé le … » et propose « Redemander ». Une
    // erreur générique ferait douter du hub, et un bouton qui échoue en
    // silence ferait recliquer trois fois.
    vi.stubGlobal(
      'fetch',
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              error: 'le fichier de ce rapport a été purgé',
              state: 'purged',
              purged_at: 1_701_204_920,
            }),
            { status: 410, headers: { 'Content-Type': 'application/json' } },
          ),
      ),
    );
    const refus = await fetchReportFile('r1').catch((e) => e);
    expect(refus).toBeInstanceOf(ApiError);
    expect(refus.status).toBe(410);
    expect(purgedAt(refus)).toBe(1_701_204_920);
  });

  it('traduit un hub injoignable comme le reste du client', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        throw new TypeError('failed to fetch');
      }),
    );
    await expect(fetchReportFile('r1')).rejects.toMatchObject({ status: 0 });
  });

  it('demande le fichier sur la route du rapport, identifiant échappé', async () => {
    const empreinte = await empreinteDe(CLASSEUR);
    const spy = serveFile(CLASSEUR, {
      'Content-Length': String(CLASSEUR.length),
      'X-LanProbe-Sha256': empreinte,
    });
    await fetchReportFile('r/1');
    expect(spy.mock.calls[0][0]).toBe('/api/reports/r%2F1/file');
  });
});

describe('reportErrorMessage', () => {
  /** Le catalogue rend la clé et ses valeurs : le test lit ce qui sera affiché. */
  const t = (key: string, values?: Record<string, string | number>) =>
    values ? `${key} ${JSON.stringify(values)}` : key;

  it('dit LES DEUX nombres sur un fichier tronqué', () => {
    // 🔴 « erreur réseau » ne permet pas de décider s'il faut réessayer ou
    // appeler ; « attendu 248 013 o · reçu 91 220 o », si.
    const msg = reportErrorMessage(
      new ReportIntegrityError({ kind: 'length', expected: 248_013, received: 91_220 }),
      t,
      'en',
    );
    expect(msg).toContain('report.hub_truncated');
    expect(msg).toContain('248,013');
    expect(msg).toContain('91,220');
  });

  it('nomme une empreinte qui ne correspond pas, et ce qu’on n’a pas pu vérifier', () => {
    expect(
      reportErrorMessage(new ReportIntegrityError({ kind: 'digest', expected: 'a', received: 'b' }), t, 'fr'),
    ).toBe('report.hub_corrupted');
    expect(reportErrorMessage(new ReportIntegrityError({ kind: 'unverifiable' }), t, 'fr')).toBe(
      'report.hub_unverifiable',
    );
  });

  it('dit la DATE de purge, pas une erreur générique', () => {
    // ⚠️ Un rapport purgé n'est pas une panne : la ligne existe, l'écran vient
    // de l'afficher, et le bon geste est « Redemander ».
    const msg = reportErrorMessage(
      new ApiError(410, 'le fichier de ce rapport a été purgé', {
        state: 'purged',
        purged_at: 1_701_204_920,
      }),
      t,
      'fr',
    );
    expect(msg).toContain('report.hub_purged_hint');
    expect(msg).toContain('2023');
  });

  it('ne dit RIEN quand l’écran s’est simplement fermé', () => {
    // Un abandon n'est pas un échec : afficher un message rouge après une
    // fermeture volontaire fait chercher une panne qui n'existe pas.
    expect(reportErrorMessage(new ReportAborted(), t, 'fr')).toBe('');
  });

  it('reprend le refus du hub tel quel — il nomme la règle', () => {
    expect(reportErrorMessage(new ApiError(400, 'aucune sonde à mettre dans ce rapport'), t, 'fr')).toBe(
      'aucune sonde à mettre dans ce rapport',
    );
    expect(reportErrorMessage(new Error('boum'), t, 'fr')).toBe('boum');
  });
});
