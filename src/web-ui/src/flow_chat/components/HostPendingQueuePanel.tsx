import { pendingQueueManager } from '../services/flow-chat-manager/PendingQueueModule';
import type { QueuedMessage } from '../types/flow-chat';
import { useEffect, useState, useSyncExternalStore } from 'react';
import { useI18n } from '@/infrastructure/i18n';
import { Button } from '@openbitfun/ui';
import { HostDialogQueue, observeHostQueue, type QueueOutboxRecord, type QueueItem } from '../../../../shared/dialog-queue/HostDialogQueue';
import {
  ChatComposerQueue, ChatComposerQueueHeader, ChatComposerQueueTitle,
  ChatComposerQueueList, ChatComposerQueueItem, ChatComposerQueueItemContent, ChatComposerQueueItemActions,
} from '@openbitfun/ui/flow-chat';

export function HostPendingQueuePanel({ queue, onRestore }: { queue: HostDialogQueue; onRestore: (item: QueuedMessage) => boolean }) {
  const { t } = useI18n('flow-chat');
  const view = useSyncExternalStore(queue.subscribe, queue.getSnapshot, queue.getSnapshot);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => observeHostQueue(queue), [queue]);
  const run = async (operation: () => Promise<unknown>) => {
    setBusy(true); setError(null);
    try { await operation(); } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  const restoreDraft = async (saved: QueueOutboxRecord, knownItem?: QueueItem) => {
    if (saved.request.action !== 'submit') return;
    await queue.prepareRestore(saved);
    const message = saved.request.message;
    const item = knownItem ?? await queue.receipt(message.turnId, saved.request.queueEpoch);
    if (!item) throw new Error(t('hostQueue.unknown'));
    if (item.status !== 'cancelled') {
      const result = await queue.act(item, 'cancel');
      if (result.receipt?.status !== 'cancelled') throw new Error(t('hostQueue.unknown'));
    }
    const cache = saved.draft as Partial<QueuedMessage> | undefined;
    const restored = onRestore({ ...cache, id: item.turnId, sessionId: queue.sessionId,
      content: message.content, displayMessage: message.displayContent, agentType: message.agentType,
      timestamp: item.createdAtMs, status: 'queued', retryCount: cache?.retryCount ?? 0,
      userMessageMetadata: message.metadata });
    if (!restored) {
      // Preserve the complete draft if the composer changed during cancellation.
      pendingQueueManager.enqueue({ ...cache, sessionId: queue.sessionId,
        content: message.content, displayMessage: message.displayContent,
        agentType: message.agentType, userMessageMetadata: message.metadata,
        retryCount: 1, initialStatus: 'failed' });
    }
    await queue.dismiss(saved);
  };
  const items = view.snapshot?.items ?? [];
  const visibleError = error || (view.error?.includes('Session is not loaded') ? null : view.error);
  if (!items.length && !view.pending.length && !visibleError) return null;
  return <ChatComposerQueue aria-label={t('hostQueue.title')}>
    <ChatComposerQueueHeader><ChatComposerQueueTitle count={items.length}>{t('hostQueue.title')}</ChatComposerQueueTitle></ChatComposerQueueHeader>
    <p>{t('hostQueue.memoryNotice')}</p>
    {visibleError && <div role="alert">{visibleError}<Button size="sm" disabled={busy} onClick={() => void run(() => queue.refresh())}>{t('hostQueue.refresh')}</Button></div>}
    <ChatComposerQueueList>
      {items.map(item => <ChatComposerQueueItem key={item.turnId} state={item.status === 'blocked' ? 'failed' : item.status === 'steering_pending' ? 'sending' : 'default'}>
        <ChatComposerQueueItemContent>
          <div>{item.displayContent}</div>
          <span>{item.status === 'blocked' ? t('hostQueue.blocked') : item.status === 'steering_pending' ? t('hostQueue.steeringPending') : t('hostQueue.queued')}</span>
          {item.attachmentCount > 0 && <span>{t('hostQueue.attachments', { count: item.attachmentCount })}</span>}
          {item.reason && <p>{item.reason}</p>}
        </ChatComposerQueueItemContent>
        <ChatComposerQueueItemActions>
          <Button size="sm" disabled={busy || !!view.error || item.status === 'steering_pending'} onClick={() => void run(async () => {
            const saved = await queue.savedDraft(item.turnId);
            if (!saved || saved.request.action !== 'submit') throw new Error(t('hostQueue.noDraft'));
            await restoreDraft(saved, item);
          })}>{t('hostQueue.edit')}</Button>
          <Button size="sm" disabled={busy || !!view.error || item.status === 'steering_pending'} onClick={() => void run(() => queue.act(item, 'promote'))}>{t('hostQueue.sendNow')}</Button>
          <Button size="sm" disabled={busy || !!view.error || item.status === 'steering_pending'} onClick={() => void run(() => queue.act(item, 'cancel'))}>{t('hostQueue.cancel')}</Button>
        </ChatComposerQueueItemActions>
      </ChatComposerQueueItem>)}
      {view.pending.map(record => <ChatComposerQueueItem key={record.key} state="failed">
        <ChatComposerQueueItemContent>{t('hostQueue.unknown')}
          {record.request.action === 'submit' && <div>{record.request.message.displayContent ?? record.request.message.content}</div>}
        </ChatComposerQueueItemContent>
        <ChatComposerQueueItemActions>
          {record.restoreIntent && record.request.action === 'submit'
            ? <Button size="sm" disabled={busy} onClick={() => void run(() => restoreDraft(record))}>{t('hostQueue.edit')}</Button>
            : <Button size="sm" disabled={busy} onClick={() => void run(() => queue.retry(record))}>{t('hostQueue.checkRetry')}</Button>}
          <Button size="sm" disabled={busy} onClick={() => void run(() => queue.dismiss(record))}>{t('hostQueue.dismiss')}</Button>
        </ChatComposerQueueItemActions>
      </ChatComposerQueueItem>)}
    </ChatComposerQueueList>
  </ChatComposerQueue>;
}
