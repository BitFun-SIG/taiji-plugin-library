/** A QR identifies a device; only the signed-in account directory grants access. */
export function accountDeviceIdFromLink(value: string): string {
  const text = value.trim();
  const prefix = 'https://remote.openbitfun.com/v/1.0.0/#/pair?';
  if (text.length > 8192 || !text.startsWith(prefix)) {
    return '';
  }
  const entries = text.slice(prefix.length).split('&');
  let id = '';
  let found = false;
  try {
    for (const entry of entries) {
      const parts = entry.split('=');
      if (decodeURIComponent(parts[0]) !== 'did') {
        continue;
      }
      if (found || parts.length !== 2) {
        return '';
      }
      found = true;
      id = decodeURIComponent(parts[1]);
    }
  } catch (_err) {
    return '';
  }
  return /^[A-Za-z0-9_.:-]{1,128}$/.test(id) ? id : '';
}
