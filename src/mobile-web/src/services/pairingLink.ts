import { OFFICIAL_RELAY_URL } from './CloudAccountClient';

export function normalizeRelayUrl(value: string): string | null {
  try {
    const normalized = value
      .replace(/^wss:\/\//, 'https://')
      .replace(/^ws:\/\//, 'http://')
      .replace(/\/ws\/?$/, '')
      .replace(/\/$/, '');
    const url = new URL(normalized);
    if (!['http:', 'https:'].includes(url.protocol)
      || !url.hostname
      || url.username
      || url.password
      || url.search
      || url.hash) {
      return null;
    }
    return url.toString().replace(/\/$/, '');
  } catch {
    return null;
  }
}

export function validPairingSecret(room: string | null, publicKey: string | null): boolean {
  return !!room
    && room.length <= 128
    && /^[A-Za-z0-9_-]+$/.test(room)
    && !['_store', 'page-data', 'pages'].includes(room)
    && !!publicKey
    && publicKey.length <= 512
    && /^[A-Za-z0-9+/=_-]+$/.test(publicKey);
}

/** QR data selects a device; it never supplies a server or encryption key. */
export function accountDeviceIdFromHash(hash: string): string | null {
  if (!hash.startsWith('#/pair?')) return null;
  const params = new URLSearchParams(hash.slice('#/pair?'.length));
  const ids = params.getAll('did');
  if (ids.length !== 1 || !/^[A-Za-z0-9_.-]{1,128}$/.test(ids[0]) || ['.', '..'].includes(ids[0])) return null;
  return ids[0];
}

/** Validate a Desktop-generated remote-control URL before navigating to it. */
export function parseScannedPairingLink(value: string, baseHref = window.location.href): string | null {
  try {
    const url = new URL(value.trim(), baseHref);
    const official = new URL(OFFICIAL_RELAY_URL);
    if (url.origin !== official.origin || url.username || url.password || url.search
      || url.pathname.replace(/\/$/, '') !== official.pathname) return null;
    const id = accountDeviceIdFromHash(url.hash);
    return id ? `${OFFICIAL_RELAY_URL}/#/pair?did=${encodeURIComponent(id)}` : null;
  } catch {
    return null;
  }
}
