import type { ReasoningPresetDescriptor } from '@/infrastructure/config/types';

type Translate = (key: string, options?: Record<string, unknown>) => string;

export function presetLabel(
  preset: ReasoningPresetDescriptor,
  t: Translate,
): string {
  return t(`reasoningEffort.${preset.id}`, { defaultValue: preset.label || preset.id });
}

export function presetDisplayLabel(
  preset: ReasoningPresetDescriptor,
  t: Translate,
): string {
  const fallback = presetLabel(preset, t);
  const semanticKey = presetSemanticKey(preset);
  return semanticKey
    ? t(`reasoningSelector.levels.${semanticKey === 'on' ? 'low' : semanticKey}`, { defaultValue: fallback })
    : fallback;
}

type ReasoningPresetSemanticKey =
  | 'off'
  | 'on'
  | 'minimal'
  | 'low'
  | 'medium'
  | 'high'
  | 'xhigh'
  | 'max';

const REASONING_PRESET_SEMANTIC_KEYS = new Set<ReasoningPresetSemanticKey>([
  'off',
  'on',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
]);

function asReasoningPresetSemanticKey(value: string): ReasoningPresetSemanticKey | undefined {
  const normalized = value.trim().toLowerCase();
  if (normalized === 'none') return 'off';
  return REASONING_PRESET_SEMANTIC_KEYS.has(normalized as ReasoningPresetSemanticKey)
    ? normalized as ReasoningPresetSemanticKey
    : undefined;
}

function presetSemanticKey(
  preset: ReasoningPresetDescriptor,
): ReasoningPresetSemanticKey | undefined {
  const idKey = asReasoningPresetSemanticKey(preset.id);
  if (idKey) return idKey;

  // Generated presets can prefix or replace their semantic id (for example
  // effort-off and budget-max). Their action is the stable meaning to localize.
  // Custom presets keep their authored label instead of being renamed by an
  // implementation detail in their request actions.
  if (preset.source === 'model_config') return undefined;

  for (const action of preset.actions) {
    if (action.type === 'toggle') return action.enabled ? 'on' : 'off';
    if (action.type === 'effort') {
      const effortKey = asReasoningPresetSemanticKey(action.value);
      if (effortKey) return effortKey;
    }
    if (action.type === 'budget_tokens') {
      if (preset.id.toLowerCase().includes('max')) return 'max';
      if (preset.id.toLowerCase().includes('high')) return 'high';
    }
  }

  return undefined;
}

/** Collapse the enabled toggle into Low without changing persisted or wire ids. */
export function reasoningPresetChoices(presets: ReasoningPresetDescriptor[]): ReasoningPresetDescriptor[] {
  return presets.some(preset => presetSemanticKey(preset) === 'low')
    ? presets.filter(preset => presetSemanticKey(preset) !== 'on')
    : presets;
}

export function resolveReasoningPresetChoice(
  preset: ReasoningPresetDescriptor | undefined,
  presets: ReasoningPresetDescriptor[],
): ReasoningPresetDescriptor | undefined {
  return preset && presetSemanticKey(preset) === 'on'
    ? presets.find(candidate => presetSemanticKey(candidate) === 'low') ?? preset
    : preset;
}

/** Fixed slider positions; use the advertised enabled toggle when no Low effort exists. */
export function reasoningSliderPresets(
  presets: ReasoningPresetDescriptor[],
): Array<ReasoningPresetDescriptor | undefined> {
  const byMeaning = new Map<ReasoningPresetSemanticKey, ReasoningPresetDescriptor>();
  for (const preset of presets) {
    const meaning = presetSemanticKey(preset);
    if (meaning && !byMeaning.has(meaning)) byMeaning.set(meaning, preset);
  }
  // Custom presets retain their named picker. Toggle-only models expose Off/Low
  // while still sending the original off/on ids to their provider.
  return [
    byMeaning.get('off'),
    byMeaning.get('low') ?? byMeaning.get('on') ?? byMeaning.get('minimal'),
    byMeaning.get('medium'),
    byMeaning.get('high'),
    byMeaning.get('xhigh') ?? byMeaning.get('max'),
  ];
}
