import React, { useEffect, useRef, useState } from 'react';
import PairingForm from '../components/PairingForm';
import { accountDeviceIdFromHash, parseScannedPairingLink } from '../services/pairingLink';
import QrScannerSheet from '../components/QrScannerSheet';
import { useI18n } from '../i18n';
import { CloudAccountClient, OFFICIAL_RELAY_URL, type CloudAccountSession } from '../services/CloudAccountClient';
import { loadMatchingCloudAccountSession, saveCloudAccountSession } from '../services/CloudAccountSessionStore';
import { RelayHttpClient } from '../services/RelayHttpClient';
import { RemoteSessionManager } from '../services/RemoteSessionManager';
import { loadMobileNavigation, type PairedNavigation } from '../services/MobileNavigationStore';
import { useMobileStore } from '../services/store';

interface PairingPageProps {
  onPaired: (client: RelayHttpClient, sessionMgr: RemoteSessionManager,
    preferredDeviceId?: string, navigation?: PairedNavigation) => void;
}

function installId(): string {
  const key = 'openbitfun.mobile.install_id';
  const existing = localStorage.getItem(key);
  if (existing) return existing;
  const created = crypto.randomUUID();
  localStorage.setItem(key, created);
  return created;
}

function routeKey(): string { return `${window.location.pathname}${window.location.hash}`; }

const PairingPageContent: React.FC<PairingPageProps> = ({ onPaired }) => {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [scannerOpen, setScannerOpen] = useState(false);
  const generation = useRef(0);
  const pending = useRef<AbortController | null>(null);
  const popup = useRef<Window | null>(null);
  const onPairedRef = useRef(onPaired);
  onPairedRef.current = onPaired;
  const targetDeviceId = accountDeviceIdFromHash(window.location.hash) || undefined;

  const connect = (session: CloudAccountSession, controllerDeviceId: string, restore: boolean) => {
    const client = new RelayHttpClient(OFFICIAL_RELAY_URL, '');
    client.installDirectAccountIdentity({ ...session, deviceId: controllerDeviceId });
    saveCloudAccountSession({ relayUrl: OFFICIAL_RELAY_URL, username: session.userId,
      controllerDeviceId, session });
    const store = useMobileStore.getState();
    store.resetForDeviceSwitch();
    store.setAuthenticatedUserId(session.userId);
    store.setAuthenticatedUserLabel(session.userId);
    store.setControlTarget(null);
    store.setConnectionStatus('paired');
    const scope = { accountId: session.userId, controllerDeviceId,
      relayUrl: OFFICIAL_RELAY_URL, routeKey: routeKey() };
    const navigation = restore ? loadMobileNavigation(scope) : null;
    session.masterKey.fill(0);
    onPairedRef.current(client, new RemoteSessionManager(client),
      targetDeviceId || navigation?.deviceId || undefined, { scope, restored: navigation });
  };

  useEffect(() => {
    // Only scanning a target resumes the tab's authenticated controller.
    if (targetDeviceId) {
      const id = installId();
      const saved = loadMatchingCloudAccountSession(OFFICIAL_RELAY_URL, '', id);
      if (saved) connect(saved.session, id, true);
    }
    return () => {
      generation.current += 1;
      pending.current?.abort();
      popup.current?.close();
    };
    // Each QR route remounts this component and owns one connection attempt.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const signIn = async () => {
    if (busy) return;
    const authWindow = window.open('about:blank', '_blank');
    if (!authWindow) { setError(t('pairing.allowSignInPopup')); return; }
    authWindow.opener = null;
    popup.current = authWindow;
    const attempt = ++generation.current;
    const controller = new AbortController();
    pending.current = controller;
    setBusy(true); setError(null);
    try {
      const account = new CloudAccountClient();
      const accessToken = await account.authorize(authWindow, controller.signal);
      if (generation.current !== attempt) return;
      const id = installId();
      const session = await account.login(accessToken, id);
      if (generation.current !== attempt) { session.masterKey.fill(0); return; }
      connect(session, id, false);
    } catch (cause) {
      if (generation.current === attempt) setError(cause instanceof Error ? cause.message : t('pairing.loginFailed'));
    } finally {
      authWindow.close();
      if (generation.current === attempt) { setBusy(false); pending.current = null; }
    }
  };

  const cancel = () => {
    generation.current += 1;
    pending.current?.abort(); popup.current?.close();
    setBusy(false);
  };

  return <div className="pairing-page"><div className="pairing-page__shell">
    <aside className="pairing-page__hero"><div className="pairing-page__hero-copy">
      <div className="pairing-page__eyebrow">{t('pairing.secureRemote')}</div>
      <h2>{t('pairing.heroTitle')}</h2><p>{t('pairing.heroDescription')}</p>
    </div></aside>
    <section className="pairing-page__panel">
      <PairingForm busy={busy} error={error} onSignIn={() => void signIn()} onCancel={cancel}
        onOpenScanner={() => setScannerOpen(true)} />
    </section>
  </div>{scannerOpen && <QrScannerSheet onClose={() => setScannerOpen(false)} onDetected={(url) => {
    const trustedLink = parseScannedPairingLink(url);
    if (!trustedLink) { setError(t('pairing.invalidScannedCode')); return; }
    setScannerOpen(false);
    window.location.assign(trustedLink);
  }} />}</div>;
};

const PairingPage: React.FC<PairingPageProps> = (props) => {
  const [route, setRoute] = useState(routeKey);
  useEffect(() => {
    const change = () => setRoute(routeKey());
    window.addEventListener('hashchange', change);
    return () => window.removeEventListener('hashchange', change);
  }, []);
  return <PairingPageContent key={route} {...props} />;
};
export default PairingPage;
