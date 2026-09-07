import type { AppearanceSurfaceDescriptor } from '@/infrastructure/appearance';

const CHAT_CONTEXT_PICKER_PARTS = [
  { id: 'root' }, { id: 'header' }, { id: 'content' },
  { id: 'currentDirectoryPath' }, { id: 'currentViewLabel' },
  { id: 'skillDescription' }, { id: 'footer' },
] as const;

const CHAT_CONTEXT_PICKER_STATES = [
  { id: 'loading', selector: { kind: 'self', suffix: '[data-openbitfun-state~="loading"]' } },
  { id: 'error', selector: { kind: 'self', suffix: '[data-openbitfun-state~="error"]' } },
] as const;

export const chatContextPickerAppearanceDescriptor: AppearanceSurfaceDescriptor = {
  id: 'chat-context-picker',
  parts: CHAT_CONTEXT_PICKER_PARTS,
  states: CHAT_CONTEXT_PICKER_STATES,
};

/** Reads pre-rename Appearance packages while targeting the current DOM contract. */
export const legacyFileMentionPickerAppearanceDescriptor: AppearanceSurfaceDescriptor = {
  id: 'file-mention-picker',
  hostSelectorId: 'chat-context-picker',
  parts: CHAT_CONTEXT_PICKER_PARTS,
  states: CHAT_CONTEXT_PICKER_STATES,
};
