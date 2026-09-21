import React, { useEffect, useState, useSyncExternalStore } from 'react';
import { MobileButton, MobileBanner } from '@openbitfun/ui/mobile';
import { HostDialogQueue, observeHostQueue } from '../../../shared/dialog-queue/HostDialogQueue';
import { useI18n } from '../i18n';
import '../styles/host-queue.scss';

export function MobileHostQueue({ queue, onRestore }: { queue: HostDialogQueue; onRestore: (text: string) => void }) {
  const { t } = useI18n();
  const view = useSyncExternalStore(queue.subscribe, queue.getSnapshot, queue.getSnapshot);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => observeHostQueue(queue), [queue]);
  const run = async (action: () => Promise<unknown>) => {
    setBusy(true); setError(null);
    try { await action(); } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  if (!view.snapshot?.items.length && !view.pending.length && !view.error && !error) return null;
  return <section className="host-message-queue" aria-label={t('queue.title')}>
    <strong>{t('queue.title')}</strong>
    <p>{t('queue.memoryNotice')}</p>
    {(error || view.error) && <MobileBanner tone="danger" role="alert">{error || view.error}</MobileBanner>}
    {(error || view.error) && <MobileButton size="sm" disabled={busy} onClick={() => void run(() => queue.refresh())}>{t('queue.refresh')}</MobileButton>}
    <ul>
      {view.snapshot?.items.map(item => <li key={item.turnId}>
        <p className="host-message-queue__preview">{item.displayContent}</p>
        <span>{item.status === 'steering_pending' ? t('queue.steeringPending') : item.status === 'blocked' ? t('queue.blocked') : t('queue.queued')}</span>
        {item.attachmentCount > 0 && <span>{t('queue.attachments', { count: item.attachmentCount })}</span>}
        {item.reason && <p>{item.reason}</p>}
        <div className="host-message-queue__actions">
          <MobileButton size="sm" disabled={busy || !!view.error || item.status === 'steering_pending'} onClick={() => void run(() => queue.act(item, 'promote'))}>{t('queue.sendNow')}</MobileButton>
          <MobileButton size="sm" disabled={busy || !!view.error || item.status === 'steering_pending'} onClick={() => void run(() => queue.act(item, 'cancel'))}>{t('queue.cancel')}</MobileButton>
        </div>
      </li>)}
      {view.pending.map(record => <li key={record.key}>
        <p>{t('queue.unknown')}</p>
        {record.request.action === 'submit' && <p className="host-message-queue__preview">{record.request.message.displayContent ?? record.request.message.content}</p>}
        <div className="host-message-queue__actions">
          <MobileButton size="sm" disabled={busy} onClick={() => void run(() => queue.retry(record))}>{t('queue.checkRetry')}</MobileButton>
          {record.request.action === 'submit' && <MobileButton size="sm" disabled={busy} onClick={() => {
            if (record.request.action === 'submit') onRestore(record.request.message.content);
          }}>{t('queue.copyDraft')}</MobileButton>}
          <MobileButton size="sm" disabled={busy} onClick={() => void run(() => queue.dismiss(record))}>{t('queue.dismiss')}</MobileButton>
        </div>
      </li>)}
    </ul>
  </section>;
}
