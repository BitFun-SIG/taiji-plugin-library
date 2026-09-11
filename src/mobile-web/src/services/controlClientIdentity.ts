let identity: { id: string; name: string } | undefined;
const STORAGE_KEY = 'openbitfun.mobile.control_client_id';

function tabClientId(): string {
  let storage: Storage | undefined;
  try {
    storage = window.sessionStorage;
    const saved = storage.getItem(STORAGE_KEY);
    if (saved && /^[a-f0-9]{32}$/.test(saved)) return saved;
  } catch { /* Keep a page identity when browser storage is unavailable. */ }
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  const id = Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('');
  try { storage?.setItem(STORAGE_KEY, id); }
  catch { /* Keep the in-memory identity even if persistence fails. */ }
  return id;
}

/** One identity per browser tab, retained across reloads and target/reconnect changes. */
export function getControlClientIdentity(): { id: string; name: string } {
  if (identity) return identity;
  const id = tabClientId();
  const ua = navigator.userAgent;
  const browser = /Edg\//.test(ua) ? 'Edge'
    : /Firefox\/|FxiOS\//.test(ua) ? 'Firefox'
      : /Chrome\/|CriOS\//.test(ua) ? 'Chrome'
        : /Safari\//.test(ua) ? 'Safari' : 'Browser';
  const platform = /iPhone|iPad|iPod/.test(ua) ? 'iOS'
    : /Android/.test(ua) ? 'Android'
      : /Windows/.test(ua) ? 'Windows'
        : /Macintosh/.test(ua) ? 'macOS'
          : /Linux/.test(ua) ? 'Linux' : '';
  identity = { id, name: [browser, platform].filter(Boolean).join(' · ') };
  return identity;
}
