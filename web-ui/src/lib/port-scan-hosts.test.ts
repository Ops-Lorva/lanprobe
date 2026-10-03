import { describe, expect, it } from 'vitest';
import type { ScanHost, ScanPort } from './api';
import { portScanHosts } from './port-scan-hosts';

const host = (ip: string): ScanHost => ({ ip });
const port = (ip: string, p: number): ScanPort => ({ ip, port: p, proto: 'tcp' });

describe('portScanHosts', () => {
  it('montre une machine scannée qui n’a AUCUN port ouvert', () => {
    // 🔴 C'est tout l'objet de cette fonction. Grouper par `ports` laissait
    // muette une machine sans port ouvert : elle n'apparaissait nulle part,
    // indiscernable d'une machine jamais scannée. C'est pourtant le résultat le
    // plus rassurant qu'on remette à un client.
    const rows = portScanHosts({ hosts: [host('10.0.8.7')], ports: [] });
    expect(rows).toEqual([{ ip: '10.0.8.7', ports: [] }]);
  });

  it('rattache chaque port à SA machine', () => {
    const rows = portScanHosts({
      hosts: [host('10.0.8.1'), host('10.0.8.2')],
      ports: [port('10.0.8.1', 443), port('10.0.8.2', 22), port('10.0.8.1', 80)],
    });
    expect(rows).toEqual([
      { ip: '10.0.8.1', ports: [port('10.0.8.1', 443), port('10.0.8.1', 80)] },
      { ip: '10.0.8.2', ports: [port('10.0.8.2', 22)] },
    ]);
  });

  it('n’invente aucune machine : celle qui n’a pas été scannée n’apparaît pas', () => {
    // ⚠️ « Scannée, rien d'ouvert » et « jamais scannée » sont deux choses, et
    // c'est la confusion entre les deux qu'on corrige. Une machine absente de
    // `hosts` ET sans port ouvert n'a pas été scannée : elle reste absente.
    expect(portScanHosts({ hosts: [], ports: [] })).toEqual([]);
  });

  it('garde une machine dont on ne connaît QUE les ports', () => {
    // ⚠️ Une sonde d'avant la v28 publie ses ports sans publier ses machines :
    // partir strictement de `hosts` ferait disparaître de l'écran et du
    // classeur des ports ouverts bel et bien relevés.
    const rows = portScanHosts({ hosts: [], ports: [port('10.0.8.9', 8080)] });
    expect(rows).toEqual([{ ip: '10.0.8.9', ports: [port('10.0.8.9', 8080)] }]);
  });

  it('suit l’ordre de la sonde : ses machines, puis ce que seuls les ports révèlent', () => {
    // ⚠️ L'ordre n'est pas trié ici : le classeur veut celui de la sonde, et
    // l'écran applique le sien par-dessus. Trier au fond changerait l'ordre des
    // lignes d'un document déjà remis à des clients.
    const rows = portScanHosts({
      hosts: [host('10.0.8.2'), host('10.0.8.1')],
      ports: [port('10.0.8.9', 80), port('10.0.8.1', 443)],
    });
    expect(rows.map((r) => r.ip)).toEqual(['10.0.8.2', '10.0.8.1', '10.0.8.9']);
  });

  it('accepte un scan absent — jamais lancé, ou encore en chargement', () => {
    expect(portScanHosts(null)).toEqual([]);
    expect(portScanHosts(undefined)).toEqual([]);
    expect(portScanHosts({})).toEqual([]);
  });

  it('résout tout en une passe, pour que la page ne paie pas une recherche par ligne', () => {
    // ⚠️ Un scan d'un /24 donne des centaines de machines : filtrer les ports
    // machine par machine ferait un parcours de la liste des ports par ligne
    // affichée.
    const rows = portScanHosts({
      hosts: Array.from({ length: 200 }, (_, i) => host(`10.0.8.${i}`)),
      ports: Array.from({ length: 200 }, (_, i) => port(`10.0.8.${i}`, 1000 + i)),
    });
    expect(rows).toHaveLength(200);
    expect(rows[199]).toEqual({ ip: '10.0.8.199', ports: [port('10.0.8.199', 1199)] });
  });
});
