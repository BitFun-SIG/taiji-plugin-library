import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

function readSource(relativePath: string): string {
  return readFileSync(
    fileURLToPath(new URL(relativePath, import.meta.url)),
    'utf8',
  ).replace(/\r\n?/g, '\n');
}

describe('FlowChat transcript column axis', () => {
  it('reserves the transcript scrollbar gutter on both edges of the scroller', () => {
    const stylesheet = readSource('./VirtualMessageList.scss');

    // Both edges, because the reading column is centred inside the scroller's
    // content box while the composer is centred on the panel itself. A
    // trailing-edge gutter alone sets the column half a scrollbar to the leading
    // side of the composer card, which is the offset that made the two disagree.
    expect(stylesheet).toContain('scrollbar-gutter: stable both-edges;');
    expect(stylesheet).not.toMatch(/scrollbar-gutter: stable;/);
  });

  it('centres the composer card on the panel and sizes it as the reading column', () => {
    const chatInput = readSource('../ChatInput.scss');
    const transcriptLayout = readSource('../../_transcript-layout.scss');

    expect(chatInput).toMatch(
      /\.openbitfun-context-drop-zone\.openbitfun-chat-input-drop-zone \{[\s\S]*?left: 50%;[\s\S]*?transform: translateX\(-50%\);[\s\S]*?max-width: 900px;/,
    );
    expect(transcriptLayout).toMatch(
      /@mixin reading-column \{[\s\S]*?max-width: 900px;/,
    );
  });
});
