// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Passkeys in the browser: converts between the server's JSON (binary
// values as base64url, as webauthn-rs sends and expects them) and the
// browser's WebAuthn API (ArrayBuffers).

/** The server's options: `{ publicKey: {...} }` with base64url values. */
export type JsonOptions = { publicKey: Record<string, unknown> } & Record<string, unknown>;

export function toBase64url(buffer: ArrayBuffer | ArrayBufferView): string {
	const bytes =
		buffer instanceof ArrayBuffer
			? new Uint8Array(buffer)
			: new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
	let s = '';
	for (const b of bytes) s += String.fromCharCode(b);
	return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

export function fromBase64url(text: string): ArrayBuffer {
	const b64 = text.replace(/-/g, '+').replace(/_/g, '/');
	const bin = atob(b64 + '='.repeat((4 - (b64.length % 4)) % 4));
	const bytes = new Uint8Array(bin.length);
	for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
	return bytes.buffer;
}

type Descriptor = { id: string; type: string; transports?: string[] };

function descriptors(list: unknown): PublicKeyCredentialDescriptor[] | undefined {
	if (!Array.isArray(list)) return undefined;
	return (list as Descriptor[]).map((d) => ({
		...d,
		id: fromBase64url(d.id),
		type: 'public-key',
		transports: d.transports as AuthenticatorTransport[] | undefined
	}));
}

/** True if the browser can use passkeys at all. */
export function passkeysSupported(): boolean {
	return typeof window !== 'undefined' && 'PublicKeyCredential' in window;
}

/** `navigator.credentials.create()` with the server's options; the result as JSON. */
export async function createPasskey(options: JsonOptions): Promise<unknown> {
	const pk = options.publicKey as Record<string, unknown> & {
		challenge: string;
		user: { id: string; name: string; displayName: string };
		excludeCredentials?: Descriptor[];
	};
	const publicKey = {
		...pk,
		challenge: fromBase64url(pk.challenge),
		user: { ...pk.user, id: fromBase64url(pk.user.id) },
		excludeCredentials: descriptors(pk.excludeCredentials)
	} as PublicKeyCredentialCreationOptions;
	const cred = (await navigator.credentials.create({ publicKey })) as PublicKeyCredential | null;
	if (!cred) throw new Error('no passkey was created');
	const response = cred.response as AuthenticatorAttestationResponse;
	return {
		id: cred.id,
		rawId: toBase64url(cred.rawId),
		type: cred.type,
		response: {
			attestationObject: toBase64url(response.attestationObject),
			clientDataJSON: toBase64url(response.clientDataJSON),
			transports: response.getTransports?.() ?? []
		},
		extensions: cred.getClientExtensionResults()
	};
}

/** `navigator.credentials.get()` with the server's options; the result as JSON. */
export async function getPasskey(options: JsonOptions): Promise<unknown> {
	const pk = options.publicKey as Record<string, unknown> & {
		challenge: string;
		allowCredentials?: Descriptor[];
	};
	const publicKey = {
		...pk,
		challenge: fromBase64url(pk.challenge),
		allowCredentials: descriptors(pk.allowCredentials)
	} as PublicKeyCredentialRequestOptions;
	const cred = (await navigator.credentials.get({ publicKey })) as PublicKeyCredential | null;
	if (!cred) throw new Error('no passkey was used');
	const response = cred.response as AuthenticatorAssertionResponse;
	return {
		id: cred.id,
		rawId: toBase64url(cred.rawId),
		type: cred.type,
		response: {
			authenticatorData: toBase64url(response.authenticatorData),
			clientDataJSON: toBase64url(response.clientDataJSON),
			signature: toBase64url(response.signature),
			userHandle: response.userHandle ? toBase64url(response.userHandle) : null
		},
		extensions: cred.getClientExtensionResults()
	};
}

/** A user-facing message for a failed browser passkey call. */
export function passkeyErrorText(e: unknown): string {
	if (e instanceof DOMException) {
		if (e.name === 'NotAllowedError') return 'The passkey request was cancelled or timed out.';
		if (e.name === 'InvalidStateError') return 'This passkey is already registered.';
		if (e.name === 'SecurityError')
			return 'Passkeys need HTTPS and the address configured as STINTORY_PUBLIC_URL.';
	}
	return e instanceof Error ? e.message : String(e);
}
