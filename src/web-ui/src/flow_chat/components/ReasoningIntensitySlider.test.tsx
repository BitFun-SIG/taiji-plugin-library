/**
 * @vitest-environment jsdom
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ReasoningPresetDescriptor } from '@/infrastructure/config/types';
import { ReasoningIntensitySlider } from './ReasoningIntensitySlider';
import { reasoningSliderPresets } from './reasoningPresetPresentation';

globalThis.IS_REACT_ACT_ENVIRONMENT = true;

vi.mock('@/infrastructure/i18n', () => ({
  useI18n: () => ({ t: (key: string) => key }),
}));

const preset = (id: string): ReasoningPresetDescriptor => ({
  id, label: id, order: 0, source: 'models_dev',
  actions: id === 'off' ? [{ type: 'toggle', enabled: false }] : [{ type: 'effort', value: id }],
});
const allPresets = ['off', 'low', 'medium', 'high', 'xhigh'].map(preset);

describe('ReasoningIntensitySlider', () => {
  let container: HTMLDivElement;
  let root: Root;
  const onSelect = vi.fn();
  const input = () => container.querySelector<HTMLInputElement>('input[type="range"]')!;
  const render = async (presets = allPresets, selected = 'medium', disabled = false) => {
    await act(async () => root.render(
      <ReasoningIntensitySlider
        presets={presets}
        selectedPreset={presets.find(item => item.id === selected)}
        onSelect={onSelect}
        disabled={disabled}
      />,
    ));
  };
  const pointer = async (type: 'pointerdown' | 'pointerup' | 'pointercancel') => {
    await act(async () => {
      input().dispatchEvent(new Event(type, { bubbles: true }));
    });
  };
  const move = async (value: number) => {
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input(), String(value));
      input().dispatchEvent(new Event('input', { bubbles: true }));
    });
  };
  const key = async (type: 'keydown' | 'keyup', value: string) => {
    await act(async () => {
      input().dispatchEvent(new KeyboardEvent(type, {
        key: value, bubbles: true, cancelable: true,
      }));
    });
  };

  beforeEach(() => {
    onSelect.mockReset();
    container = document.createElement('div');
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it('previews a drag and saves the final supported preset once on release', async () => {
    await render();
    await pointer('pointerdown');
    await move(1);
    await move(4);
    expect(input().value).toBe('4');
    expect(input().getAttribute('aria-valuetext')).toBe('reasoningSelector.levels.xhigh');
    expect(container.textContent).toBe('reasoningSelector.levels.xhigh');
    expect(onSelect).not.toHaveBeenCalled();
    await pointer('pointerup');
    await act(async () => {
      input().dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
    });
    expect(onSelect).toHaveBeenCalledExactlyOnceWith('xhigh');
  });

  it('keeps the fixed five levels and skips unsupported positions with the keyboard', async () => {
    await render([preset('medium'), preset('xhigh')]);
    expect(input().value).toBe('2');
    expect(input().min).toBe('0');
    expect(input().max).toBe('4');
    expect(input().step).toBe('1');
    expect(container.textContent).toBe('reasoningSelector.levels.medium');
    await key('keydown', 'ArrowRight');
    expect(input().value).toBe('4');
    expect(container.textContent).toBe('reasoningSelector.levels.xhigh');
    expect(onSelect).not.toHaveBeenCalled();
    await key('keyup', 'ArrowRight');
    expect(onSelect).toHaveBeenCalledExactlyOnceWith('xhigh');
    await key('keydown', 'Home');
    await key('keyup', 'Home');
    // Off is unavailable, so Home stays at Medium instead of clearing to Auto.
    expect(input().value).toBe('2');
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it('submits the advertised off preset rather than resetting to automatic', async () => {
    const off = { ...preset('off'), id: 'effort-off' };
    await render([off, preset('high')], 'high');
    await key('keydown', 'Home');
    await key('keyup', 'Home');
    expect(onSelect).toHaveBeenCalledExactlyOnceWith('effort-off');
  });

  it('snaps a pointer selection to a supported level', async () => {
    await render([preset('low'), preset('xhigh')], 'low');
    await pointer('pointerdown');
    await move(3);
    expect(input().value).toBe('4');
    await pointer('pointerup');
    expect(onSelect).toHaveBeenCalledExactlyOnceWith('xhigh');
  });

  it('discards cancelled gestures and stale drafts after a catalog refresh', async () => {
    await render();
    await pointer('pointerdown');
    await move(4);
    await pointer('pointercancel');
    expect(input().value).toBe('2');
    await pointer('pointerup');
    await key('keydown', 'ArrowRight');
    await key('keydown', 'Escape');
    await key('keyup', 'ArrowRight');
    expect(input().value).toBe('2');
    await pointer('pointerdown');
    await move(4);
    await render([preset('medium'), preset('high')]);
    await pointer('pointerup');
    expect(input().value).toBe('2');
    expect(onSelect).not.toHaveBeenCalled();
  });

  it('supports changes without pointer events and respects a disabled control', async () => {
    await render();
    await move(3);
    expect(onSelect).toHaveBeenCalledExactlyOnceWith('high');
    await render(allPresets, 'medium', true);
    await key('keydown', 'End');
    await key('keyup', 'End');
    expect(input().disabled).toBe(true);
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it('shows toggle-only reasoning as Off/Low while preserving its advertised ids', async () => {
    const on: ReasoningPresetDescriptor = {
      ...preset('on'), actions: [{ type: 'toggle', enabled: true }],
    };
    await render([preset('off'), on], 'on');
    expect(input().value).toBe('1');
    expect(input().getAttribute('aria-valuetext')).toBe('reasoningSelector.levels.low');
    await render([preset('off'), on], 'off');
    await key('keydown', 'ArrowRight');
    await key('keyup', 'ArrowRight');
    expect(onSelect).toHaveBeenCalledExactlyOnceWith('on');

    await render([preset('off'), on, preset('low'), preset('high')], 'on');
    expect(input().value).toBe('1');
    expect(input().getAttribute('aria-valuetext')).toBe('reasoningSelector.levels.low');
    await render([preset('off'), on, preset('low'), preset('high')], 'off');
    await key('keydown', 'ArrowRight');
    await key('keyup', 'ArrowRight');
    expect(onSelect).toHaveBeenLastCalledWith('low');
  });

  it('preserves catalog semantics instead of inventing levels for custom presets', async () => {
    const custom: ReasoningPresetDescriptor = {
      ...preset('custom'), source: 'model_config', actions: [{ type: 'effort', value: 'high' }],
    };
    expect(reasoningSliderPresets([preset('medium'), preset('high')]).map(item => item?.id))
      .toEqual([undefined, undefined, 'medium', 'high', undefined]);
    expect(reasoningSliderPresets([preset('minimal'), preset('max')]).map(item => item?.id))
      .toEqual([undefined, 'minimal', undefined, undefined, 'max']);
    await render([preset('off'), custom], custom.id);
    expect(input()).toBeNull();
    const openNamedPicker = vi.fn();
    await act(async () => root.render(
      <ReasoningIntensitySlider
        presets={[preset('off'), custom]}
        selectedPreset={custom}
        onSelect={onSelect}
        renderValue={label => <button onClick={openNamedPicker}>{label}</button>}
      />,
    ));
    expect(input()).toBeNull();
    const namedChoice = container.querySelector('button')!;
    expect(namedChoice.textContent).toBe('reasoningEffort.custom');
    await act(async () => namedChoice.click());
    expect(openNamedPicker).toHaveBeenCalledOnce();
    expect(onSelect).not.toHaveBeenCalled();
  });
});
