import React from 'react';
import { createRoot } from 'react-dom/client';
import ChatComposerBar from '../../src/components/ChatComposerBar';
import { MobileHostQueue } from '../../src/components/MobileHostQueue';
import { I18nProvider } from '../../src/i18n';
import { HostDialogQueue } from '../../../shared/dialog-queue/HostDialogQueue';

export function mountHostQueueFixture() {
  const element = document.createElement('main');
  document.body.replaceChildren(element);
  const calls: string[] = [];
  const queue = new HostDialogQueue('ui-fixture', 'session', async request => {
    calls.push(request.action);
    return { sessionId: 'session', queueEpoch: 'epoch', revision: calls.length, activeTurnId: 'active',
      items: [{ turnId: 'queued', content: '', displayContent: '接着检查错误处理和测试覆盖', previewTruncated: false,
        attachmentCount: 0, agentType: 'Standard', createdAtMs: 1, status: 'queued', reason: null, targetTurnId: null, steeringId: null }],
      capacity: 20, used: 1, receipt: null };
  });
  const noop = () => {};
  const root = createRoot(element);
  root.render(<I18nProvider><MobileHostQueue queue={queue} onRestore={noop} />
    <ChatComposerBar cancelling={false} containerRef={null} expanded imageAnalyzing={false} sending={false}
      input="继续检查" inputRef={null} modelControls={null} onActivate={noop} onAttach={noop} onCancel={() => calls.push('stop')}
      onChange={noop} onCompositionEnd={noop} onCompositionStart={noop} onKeyDown={noop} onRemoveImage={noop}
      onSend={() => calls.push('send')} pendingImages={[]} remoteUnavailable={false} streaming />
  </I18nProvider>);
  return { calls, dispose: () => root.unmount() };
}
