/**
 * Clés d'accès, côté navigateur.
 *
 * ⚠️ L'API WebAuthn parle en `ArrayBuffer` là où le hub parle en base64url.
 * Les conversions sont toute la difficulté de ce fichier : une seule oubliée
 * et le navigateur rejette la cérémonie avec un `TypeError` qui ne nomme rien.
 */

/** Base64url → octets. Sans remplissage : c'est la forme de WebAuthn. */
function fromBase64Url(value: string): Uint8Array {
  const padded = value.replace(/-/g, '+').replace(/_/g, '/');
  const binary = atob(padded + '='.repeat((4 - (padded.length % 4)) % 4));
  return Uint8Array.from(binary, (c) => c.charCodeAt(0));
}

/** Octets → base64url, sans remplissage. */
function toBase64Url(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = '';
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** Le navigateur autorise-t-il l'API ici ? */
export function passkeysSupported(): boolean {
  // `isSecureContext` couvre HTTPS et localhost — la règle exacte du
  // navigateur, plutôt qu'une comparaison d'URL qu'on referait de travers.
  return (
    typeof window !== 'undefined' &&
    window.isSecureContext &&
    typeof navigator !== 'undefined' &&
    'credentials' in navigator &&
    typeof PublicKeyCredential !== 'undefined'
  );
}

type Challenge = {
  publicKey: Record<string, unknown> & {
    challenge: string;
    user?: { id: string; name: string; displayName: string };
    excludeCredentials?: { id: string; type: string; transports?: string[] }[];
    allowCredentials?: { id: string; type: string; transports?: string[] }[];
  };
};

/** Crée une clé et rend ce que le hub attend en retour. */
export async function createCredential(challenge: Challenge): Promise<unknown> {
  const options = challenge.publicKey;
  const request: PublicKeyCredentialCreationOptions = {
    ...(options as unknown as PublicKeyCredentialCreationOptions),
    challenge: fromBase64Url(options.challenge) as unknown as BufferSource,
    user: {
      ...(options.user as unknown as PublicKeyCredentialUserEntity),
      id: fromBase64Url(options.user!.id) as unknown as BufferSource,
    },
    excludeCredentials: (options.excludeCredentials ?? []).map((c) => ({
      ...c,
      id: fromBase64Url(c.id) as unknown as BufferSource,
      type: 'public-key' as const,
      transports: c.transports as AuthenticatorTransport[] | undefined,
    })),
  };

  const credential = (await navigator.credentials.create({
    publicKey: request,
  })) as PublicKeyCredential | null;
  if (!credential) throw new Error('aucune clé produite');

  const response = credential.response as AuthenticatorAttestationResponse;
  return {
    id: credential.id,
    rawId: toBase64Url(credential.rawId),
    type: credential.type,
    // `extensions` est exigé par le hub même vide : l'omettre fait échouer la
    // désérialisation côté serveur avec un message qui parle de champ manquant
    // sans dire lequel.
    extensions: credential.getClientExtensionResults(),
    response: {
      attestationObject: toBase64Url(response.attestationObject),
      clientDataJSON: toBase64Url(response.clientDataJSON),
    },
  };
}

/** Signe un défi avec une clé existante. */
export async function getAssertion(challenge: Challenge): Promise<unknown> {
  const options = challenge.publicKey;
  const request: PublicKeyCredentialRequestOptions = {
    ...(options as unknown as PublicKeyCredentialRequestOptions),
    challenge: fromBase64Url(options.challenge) as unknown as BufferSource,
    allowCredentials: (options.allowCredentials ?? []).map((c) => ({
      ...c,
      id: fromBase64Url(c.id) as unknown as BufferSource,
      type: 'public-key' as const,
      transports: c.transports as AuthenticatorTransport[] | undefined,
    })),
  };

  const credential = (await navigator.credentials.get({
    publicKey: request,
  })) as PublicKeyCredential | null;
  if (!credential) throw new Error('aucune signature produite');

  const response = credential.response as AuthenticatorAssertionResponse;
  return {
    id: credential.id,
    rawId: toBase64Url(credential.rawId),
    type: credential.type,
    extensions: credential.getClientExtensionResults(),
    response: {
      authenticatorData: toBase64Url(response.authenticatorData),
      clientDataJSON: toBase64Url(response.clientDataJSON),
      signature: toBase64Url(response.signature),
      userHandle: response.userHandle ? toBase64Url(response.userHandle) : null,
    },
  };
}

/**
 * Traduit l'échec d'une cérémonie en phrase utile.
 *
 * Les `DOMException` de WebAuthn sont volontairement avares : elles ne
 * distinguent pas « clé refusée » de « mauvais domaine » pour ne pas aider un
 * site malveillant. On rend donc ce qu'on peut, sans inventer.
 */
export function ceremonyError(e: unknown, t: (k: string) => string): string {
  if (e instanceof DOMException) {
    if (e.name === 'NotAllowedError') return t('account.pk_cancelled');
    if (e.name === 'InvalidStateError') return t('account.pk_already');
    if (e.name === 'SecurityError') return t('account.pk_insecure');
  }
  return e instanceof Error ? e.message : String(e);
}
