// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as Monaco from 'monaco-editor';
import { setMonacoRuntime } from './monacoRuntime';

vi.mock('@/shared/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

import { monacoModelManager } from './MonacoModelManager';

type ChangeListener = () => void;

class FakeModel {
  private value: string;
  private versionId = 1;
  private listeners: ChangeListener[] = [];
  public readonly uri: { toString: () => string };

  constructor(value: string, uriString: string) {
    this.value = value;
    this.uri = { toString: () => uriString };
  }

  getValue(): string {
    return this.value;
  }

  setValue(next: string): void {
    if (next === this.value) return;
    this.value = next;
    this.versionId += 1;
    this.listeners.forEach(listener => listener());
  }

  getAlternativeVersionId(): number {
    return this.versionId;
  }

  onDidChangeContent(listener: ChangeListener): { dispose: () => void } {
    this.listeners.push(listener);
    return {
      dispose: () => {
        const index = this.listeners.indexOf(listener);
        if (index > -1) this.listeners.splice(index, 1);
      },
    };
  }
}

function createFakeMonaco(): typeof Monaco {
  const models = new Map<string, FakeModel>();

  const Uri = {
    file: (path: string) => ({ toString: () => `file://${path}` }),
    parse: (value: string) => ({ toString: () => value }),
  };

  const editor = {
    getModel: (uri: { toString: () => string }) => models.get(uri.toString()) ?? null,
    createModel: (content: string, _language: string, uri: { toString: () => string }) => {
      const model = new FakeModel(content, uri.toString());
      models.set(uri.toString(), model);
      return model;
    },
    onWillDisposeModel: () => ({ dispose: () => {} }),
  };

  return { Uri, editor } as unknown as typeof Monaco;
}

interface DirtyEvent {
  filePath: string;
  isDirty: boolean;
}

function listenDirtyEvents(): { events: DirtyEvent[]; dispose: () => void } {
  const events: DirtyEvent[] = [];
  const handler = (event: Event) => {
    const detail = (event as CustomEvent<DirtyEvent>).detail;
    events.push({ filePath: detail.filePath, isDirty: detail.isDirty });
  };
  window.addEventListener('monaco-model-dirty-changed', handler as EventListener);
  return {
    events,
    dispose: () => window.removeEventListener('monaco-model-dirty-changed', handler as EventListener),
  };
}

let dirtyListener: { events: DirtyEvent[]; dispose: () => void };

beforeEach(() => {
  setMonacoRuntime(createFakeMonaco());
  dirtyListener = listenDirtyEvents();
});

afterEach(() => {
  dirtyListener.dispose();
  setMonacoRuntime(null);
});

describe('MonacoModelManager external sync dirty state (issue #3165)', () => {
  it('updateModelContent with markAsSaved=true leaves the model clean', () => {
    const filePath = '/repo/external-sync-saved.ts';
    const model = monacoModelManager.getOrCreateModel(filePath, 'typescript', 'const a = 1;');
    expect(monacoModelManager.getModelMetadata(filePath)?.isDirty).toBe(false);

    // External program rewrote the file on disk; the sync path pushes the new
    // disk truth into the open model and marks it saved.
    monacoModelManager.updateModelContent(filePath, 'const a = 2;', true);

    expect(model.getValue()).toBe('const a = 2;');
    const metadata = monacoModelManager.getModelMetadata(filePath);
    expect(metadata?.isDirty).toBe(false);
  });

  it('updateModelContent with markAsSaved=true broadcasts a clean dirty-changed event', () => {
    const filePath = '/repo/external-sync-broadcast.ts';
    monacoModelManager.getOrCreateModel(filePath, 'typescript', 'const b = 1;');
    dirtyListener.events.length = 0;

    monacoModelManager.updateModelContent(filePath, 'const b = 2;', true);

    // Consumers that track the dirty dot must see the model end up clean; a
    // transient dirty flip without a clean follow-up strands the marker.
    const mine = dirtyListener.events.filter(event => event.filePath === filePath);
    expect(mine.length).toBeGreaterThan(0);
    expect(mine[mine.length - 1].isDirty).toBe(false);
    expect(mine.some(event => event.isDirty)).toBe(false);
  });

  it('a bracketed external sync does not flip the dirty flag or broadcast transient dirt', () => {
    const filePath = '/repo/external-sync-bracket.ts';
    const model = monacoModelManager.getOrCreateModel(filePath, 'typescript', 'const c = 1;');
    dirtyListener.events.length = 0;

    monacoModelManager.beginExternalSync();
    try {
      model.setValue('const c = 2;');
    } finally {
      monacoModelManager.endExternalSync();
    }
    monacoModelManager.markAsSaved(filePath);

    expect(monacoModelManager.getModelMetadata(filePath)?.isDirty).toBe(false);
    const mine = dirtyListener.events.filter(event => event.filePath === filePath);
    expect(mine.some(event => event.isDirty)).toBe(false);
    expect(mine[mine.length - 1]?.isDirty).toBe(false);
  });

  it('still marks real user edits outside a sync bracket as dirty', () => {
    const filePath = '/repo/user-edit-control.ts';
    const model = monacoModelManager.getOrCreateModel(filePath, 'typescript', 'const d = 1;');
    dirtyListener.events.length = 0;

    // Control: an edit that is not an external sync must keep the old behavior.
    model.setValue('const d = 2; // user typed');

    expect(monacoModelManager.getModelMetadata(filePath)?.isDirty).toBe(true);
    const mine = dirtyListener.events.filter(event => event.filePath === filePath);
    expect(mine.some(event => event.isDirty)).toBe(true);
  });

  it('an unbalanced endExternalSync never leaves suppression stuck on', () => {
    const filePath = '/repo/unbalanced-bracket.ts';
    const model = monacoModelManager.getOrCreateModel(filePath, 'typescript', 'const e = 1;');

    monacoModelManager.endExternalSync(); // no matching begin: must be a no-op
    dirtyListener.events.length = 0;
    model.setValue('const e = 2; // user typed');

    expect(monacoModelManager.getModelMetadata(filePath)?.isDirty).toBe(true);
  });
});
