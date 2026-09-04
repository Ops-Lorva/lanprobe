import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from './api';
import {
  PAIR_CODE_TTL_SECS,
  ageLabel,
  deviceCounts,
  deviceRows,
  devicesApi,
  pairingAddress,
  parseDevices,
  type PairedDevice,
} from './devices';

/**
 * L'appairage d'un téléphone, côté interface (contrat § 22).
 *
 * 🔴 Ce qui se teste ici n'est pas décoratif. L'identité d'appareil n'expire
 * pas : un téléphone perdu garde l'accès jusqu'à sa révocation, et rien
 * d'autre ne le coupe — ni le temps, ni un changement de mot de passe, ni un
 * redémarrage du hub. La liste et son bouton « Révoquer » sont donc le seul
 * organe qui rend le modèle acceptable. Une liste qui trie mal, qui perd une
 * ligne à cause d'une enveloppe JSON inattendue ou qui appelle la mauvaise
 * route, c'est un appareil qu'on croit révoqué et qui ne l'est pas.
 */

function device(over: Partial<PairedDevice> = {}): PairedDevice {
  return {
    device_id: 'd1',
    username: 'benjamin',
    name: 'iPhone 15',
    platform: 'iOS 18.2 · iPhone 15',
    app_version: '1.2.0',
    created_at: 1_700_000_000,
    last_seen: null,
    last_ip: null,
    revoked_at: null,
    archived_at: null,
    ...over,
  };
}

describe('parseDevices', () => {
  it('lit une liste nue', () => {
    expect(parseDevices([device({ device_id: 'a' })]).map((d) => d.device_id)).toEqual(['a']);
  });

  it('lit aussi une liste enveloppée sous `devices`', () => {
    // ⚠️ Le hub rend `501` tant que la route n'est pas écrite : l'enveloppe
    // n'est donc pas encore observable. Le contrat § 22 ne la fixe pas, et les
    // deux formes existent déjà dans ce hub — `readTokens` rend un tableau nu,
    // `passkeys` un objet. Tolérer les deux coûte quatre lignes ; se tromper
    // coûte une liste d'appareils vide devant quelqu'un qui cherche à révoquer.
    expect(parseDevices({ devices: [device({ device_id: 'b' })] }).map((d) => d.device_id)).toEqual([
      'b',
    ]);
  });

  it('rend une liste vide plutôt que de jeter sur une réponse inattendue', () => {
    // Une exception ici laisserait la carte en chargement perpétuel, sans rien
    // à lire : c'est exactement le comportement interdit face au `501`.
    expect(parseDevices(null)).toEqual([]);
    expect(parseDevices(undefined)).toEqual([]);
    expect(parseDevices('appairage non encore implémenté')).toEqual([]);
    expect(parseDevices({ error: 'boom' })).toEqual([]);
  });
});

describe('deviceRows', () => {
  it('marque révoqué ce que `revoked_at` date', () => {
    const rows = deviceRows([device({ revoked_at: 1_700_000_500 })]);
    expect(rows[0].revoked).toBe(true);
    expect(deviceRows([device()])[0].revoked).toBe(false);
  });

  it('range les appareils actifs avant les révoqués', () => {
    // Un appareil révoqué reste en base — aucune route ne supprime — mais il
    // n'est plus la question : le laisser se mêler aux actifs fait chercher
    // dans une liste où la moitié des lignes ne veut plus rien dire.
    const rows = deviceRows([
      device({ device_id: 'mort', revoked_at: 1_700_000_900, last_seen: 1_700_000_800 }),
      device({ device_id: 'vif', last_seen: 1_700_000_100 }),
    ]);
    expect(rows.map((r) => r.device.device_id)).toEqual(['vif', 'mort']);
  });

  it('met en tête celui qui vient de parler', () => {
    // La dernière vue et la dernière adresse sont le SEUL détecteur d'un
    // secret copié, puisque le secret ne tourne pas à l'usage (§ 22). Le plus
    // récent en tête, c'est la ligne qu'on vient vérifier.
    const rows = deviceRows([
      device({ device_id: 'ancien', last_seen: 1_700_000_100 }),
      device({ device_id: 'recent', last_seen: 1_700_009_000 }),
    ]);
    expect(rows.map((r) => r.device.device_id)).toEqual(['recent', 'ancien']);
  });

  it('retombe sur la date d’appairage quand l’appareil n’a jamais été vu', () => {
    const rows = deviceRows([
      device({ device_id: 'jamais_vu_recent', created_at: 1_700_009_000, last_seen: null }),
      device({ device_id: 'vu_il_y_a_longtemps', created_at: 1, last_seen: 1_700_000_100 }),
    ]);
    expect(rows.map((r) => r.device.device_id)).toEqual([
      'jamais_vu_recent',
      'vu_il_y_a_longtemps',
    ]);
  });

  it('marque masqué ce que `archived_at` date', () => {
    const rows = deviceRows([device({ revoked_at: 1_700_000_500, archived_at: 1_700_000_600 })]);
    expect(rows[0].archived).toBe(true);
    expect(deviceRows([device()])[0].archived).toBe(false);
  });

  // Les masqués n'apparaissent que sur demande explicite. Quand ils sont là,
  // ils sont EN DERNIER : on vient de demander à les revoir, pas à les mêler
  // aux lignes qui comptent.
  it('range les masqués après les révoqués visibles', () => {
    const rows = deviceRows([
      device({ device_id: 'range', revoked_at: 1_700_000_900, archived_at: 1_700_000_950 }),
      device({ device_id: 'mort', revoked_at: 1_700_000_800 }),
      device({ device_id: 'vif' }),
    ]);
    expect(rows.map((r) => r.device.device_id)).toEqual(['vif', 'mort', 'range']);
  });
});

/**
 * 🔴 Le compteur de la section « Appareils » compte ce qu'il MONTRE.
 * Annoncer « 2 appareils » en cachant trois révoqués serait un mensonge
 * tranquille — et cette section est précisément celle qu'on lit pour repérer
 * un appareil qu'on ne reconnaît pas.
 */
describe('deviceCounts', () => {
  it('compte les lignes rendues, jamais autre chose', () => {
    const rows = deviceRows([
      device({ device_id: 'vif' }),
      device({ device_id: 'autre' }),
      device({ device_id: 'mort', revoked_at: 1_700_000_800 }),
    ]);
    expect(deviceCounts(rows)).toEqual({ active: 2, revoked: 1 });
  });

  it('compte les masqués parmi les révoqués DÈS QU’ILS SONT À L’ÉCRAN', () => {
    // La bascule ouverte, ils sont affichés : les taire ferait mentir le
    // compteur dans l'autre sens.
    const rows = deviceRows([
      device({ device_id: 'vif' }),
      device({ device_id: 'range', revoked_at: 1_700_000_900, archived_at: 1_700_000_950 }),
    ]);
    expect(deviceCounts(rows)).toEqual({ active: 1, revoked: 1 });
  });

  it('ne compte rien sur une liste vide', () => {
    expect(deviceCounts([])).toEqual({ active: 0, revoked: 0 });
  });
});

describe('ageLabel', () => {
  const MAINTENANT = 1_700_000_000_000;

  it('rend une durée NUE, pas un « il y a »', () => {
    // ⚠️ La phrase est déjà dans le catalogue : « vu il y a {age} ». Y glisser
    // un `Intl.RelativeTimeFormat` donnerait « vu il y a il y a 41 j ».
    const label = ageLabel(MAINTENANT / 1000 - 41 * 86_400, 'en', MAINTENANT);
    expect(label).toContain('41');
    expect(label.toLowerCase()).not.toContain('ago');
  });

  it('choisit l’unité qui se lit, et s’arrête au jour', () => {
    expect(ageLabel(MAINTENANT / 1000 - 30, 'en', MAINTENANT)).toContain('30');
    expect(ageLabel(MAINTENANT / 1000 - 3 * 3600, 'en', MAINTENANT)).toContain('3');
    // ⚠️ « vu il y a 1 mois » ne se tranche pas ; « vu il y a 90 j », si.
    // Cette ligne est le seul endroit où l'ancienneté d'un appareil se lit :
    // il n'existe aucune révocation par inactivité pour la rattraper.
    expect(ageLabel(MAINTENANT / 1000 - 90 * 86_400, 'en', MAINTENANT)).toContain('90');
  });

  it('ne rend jamais un âge négatif', () => {
    // Une horloge de téléphone en avance sur celle du hub est banale. « vu il
    // y a -3 min » se lit comme une panne de l'écran, pas comme un décalage.
    expect(ageLabel(MAINTENANT / 1000 + 180, 'en', MAINTENANT)).not.toContain('-');
  });
});

describe('pairingAddress', () => {
  it('préfère l’adresse publique réglée sur le hub', () => {
    // C'est celle que le téléphone doit joindre. `location.origin` peut être
    // une IP interne atteignable depuis ce poste et de nulle part ailleurs.
    expect(pairingAddress('https://hub.exemple.fr', 'http://10.0.0.9:8080')).toBe(
      'https://hub.exemple.fr',
    );
  });

  it('retombe sur l’origine de la page quand rien n’est réglé', () => {
    expect(pairingAddress(null, 'http://10.0.0.9:8080')).toBe('http://10.0.0.9:8080');
    expect(pairingAddress('   ', 'http://10.0.0.9:8080')).toBe('http://10.0.0.9:8080');
  });

  it('retire la barre finale : elle se recopie à la main', () => {
    expect(pairingAddress('https://hub.exemple.fr/', 'http://x')).toBe('https://hub.exemple.fr');
  });
});

describe('PAIR_CODE_TTL_SECS', () => {
  it('vaut les 15 minutes du hub', () => {
    // ⚠️ Pas 24 h : le confort d'une fois par appareil ne paie pas une fenêtre
    // d'attaque d'une journée (§ 22). La valeur sert au repli quand le hub ne
    // renvoie pas d'échéance ; elle doit rester celle de `PAIR_CODE_TTL_SECS`
    // dans `crates/lanprobe-web/src/pairing.rs`.
    expect(PAIR_CODE_TTL_SECS).toBe(900);
  });
});

/** Dernier appel `fetch` observé. */
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

describe('devicesApi', () => {
  it('demande un code sur la route de SON compte', async () => {
    const spy = captureFetch({ code: 'ABCD1234', expires_at: 1_700_000_900 });
    await devicesApi.createPairCode();
    const [url, init] = spy.mock.calls[0];
    if (!init) throw new Error('aucune option de requête');
    expect(url).toBe('/api/me/pair-codes');
    expect(init.method).toBe('POST');
  });

  it('révoque par POST sur `/revoke`, jamais par DELETE', async () => {
    // 🔴 Aucune route ne supprime un appareil : la ligne reste, parce que le
    // journal d'audit la nomme et qu'une ligne d'audit qui pointe une cible
    // disparue ne prouve rien.
    const spy = captureFetch({ ok: true });
    await devicesApi.revokeMyDevice('7f/3');
    const [url, init] = spy.mock.calls[0];
    if (!init) throw new Error('aucune option de requête');
    expect(url).toBe('/api/me/devices/7f%2F3/revoke');
    expect(init.method).toBe('POST');
    expect(init.method).not.toBe('DELETE');
  });

  it('laisse remonter le refus du hub, message compris', async () => {
    // Les routes rendent `501` tant que l'appairage n'est pas écrit côté hub.
    // L'écran doit pouvoir DIRE ce refus : un échec avalé ici laisserait une
    // carte vide, sans rien à lire ni à comprendre.
    captureFetch({ error: 'appairage non encore implémenté' }, 501);
    await expect(devicesApi.myDevices()).rejects.toMatchObject({
      status: 501,
      message: 'appairage non encore implémenté',
    });
    await expect(devicesApi.myDevices()).rejects.toBeInstanceOf(ApiError);
  });
});
