/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import { HostDialogQueue, type QueueOutboxRecord, type QueueSnapshot } from '../../../../shared/dialog-queue/HostDialogQueue';
import { HostPendingQueuePanel } from './HostPendingQueuePanel';

vi.mock('@/infrastructure/i18n', () => ({ useI18n: () => ({ t: (key: string) => key }) }));
vi.mock('../services/flow-chat-manager/PendingQueueModule', () => ({ pendingQueueManager: { enqueue: vi.fn() } }));

it.each([false, true])('keeps a normal send hidden and shows recovery only on failure (failure=%s)', async failure => {
  const records = new Map<string, QueueOutboxRecord>();
  let finish!: () => void;
  let entered!: () => void;
  const started = new Promise<void>(resolve => { entered = resolve; });
  const acknowledgement = new Promise<void>(resolve => { finish = resolve; });
  const snapshot: QueueSnapshot = { sessionId: 'session', queueEpoch: 'epoch', revision: 0,
    activeTurnId: null, items: [], capacity: 20, used: 0, receipt: null };
  const queue = new HostDialogQueue('scope', 'session', async request => {
    if (request.action !== 'submit') return snapshot;
    entered();
    await acknowledgement;
    if (failure) throw new Error('Connection lost');
    return { ...snapshot, revision: 1, receipt: { turnId: request.message.turnId,
      displayContent: request.message.content, status: 'started', previewTruncated: false,
      attachmentCount: 0, agentType: 'Standard', createdAtMs: 1, reason: null,
      targetTurnId: null, steeringId: null } };
  }, {
    list: async () => [...records.values()],
    put: async record => { records.set(record.key, record); },
    remove: async key => { records.delete(key); },
  });
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () => root.render(<HostPendingQueuePanel queue={queue} onRestore={() => true} />));
    let sending!: Promise<unknown>;
    await act(async () => {
      sending = queue.submit({ content: 'hello', agentType: 'Standard', attachments: [], metadata: {} }).catch(() => undefined);
      await started;
      await queue.refresh();
    });
    expect(records.size).toBe(1);
    expect(container.textContent).toBe('');
    await act(async () => { finish(); await sending; });
    if (failure) {
      expect(container.textContent).toContain('hostQueue.unknown');
      expect(container.textContent).toContain('hello');
    } else {
      expect(container.textContent).toBe('');
    }
  } finally {
    finish();
    await act(async () => root.unmount());
    container.remove();
  }
});
