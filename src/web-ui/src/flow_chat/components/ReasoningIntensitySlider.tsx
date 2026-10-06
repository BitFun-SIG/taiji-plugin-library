import React, { useEffect, useRef, useState } from 'react';
import { useI18n } from '@/infrastructure/i18n';
import type { ReasoningPresetDescriptor } from '@/infrastructure/config/types';
import { presetDisplayLabel, reasoningSliderPresets, resolveReasoningPresetChoice } from './reasoningPresetPresentation';
import './ReasoningIntensitySlider.scss';

interface ReasoningIntensitySliderProps {
  presets: ReasoningPresetDescriptor[];
  selectedPreset?: ReasoningPresetDescriptor;
  title?: string;
  headerStart?: React.ReactNode;
  disabled?: boolean;
  onSelect: (presetId: string) => void | Promise<void>;
  renderValue?: (label: string) => React.ReactNode;
}

const adjustmentKeys = new Set(['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End']);

export const ReasoningIntensitySlider: React.FC<ReasoningIntensitySliderProps> = ({
  presets,
  selectedPreset,
  title,
  headerStart,
  disabled = false,
  onSelect,
  renderValue,
}) => {
  const { t } = useI18n('flow-chat');
  const stops = reasoningSliderPresets(presets);
  const supported = stops.flatMap((preset, index) => preset ? [index] : []);
  const selectedChoice = resolveReasoningPresetChoice(selectedPreset, presets);
  const selectedIndex = stops.findIndex(preset => preset && preset.id === selectedChoice?.id);
  const catalogKey = stops.map(preset => preset?.id ?? '').join('\0');
  const [draft, setDraft] = useState<number | null>(null);
  const draftRef = useRef<number | null>(null);
  const pointerActiveRef = useRef(false);

  useEffect(() => {
    draftRef.current = null;
    setDraft(null);
  }, [catalogKey, selectedPreset?.id]);

  // Keep custom presets reachable through their named picker.
  if (selectedIndex < 0 && !renderValue) return null;

  const value = draft ?? selectedIndex;
  const displayedPreset = stops[value] ?? selectedPreset;
  const label = displayedPreset ? presetDisplayLabel(displayedPreset, t) : t('reasoningSelector.auto');
  const locked = disabled || supported.length < 2;

  const cancel = () => {
    draftRef.current = null;
    setDraft(null);
  };

  const preview = (requested: number) => {
    if (locked) return;
    const previous = draftRef.current ?? selectedIndex;
    const nearest = supported.reduce((best, candidate) => {
      const distance = Math.abs(candidate - requested);
      const bestDistance = Math.abs(best - requested);
      if (distance < bestDistance) return candidate;
      if (distance === bestDistance) return requested > previous ? Math.max(best, candidate) : Math.min(best, candidate);
      return best;
    }, supported[0]);
    draftRef.current = nearest;
    setDraft(nearest);
  };

  const commit = async () => {
    const next = draftRef.current;
    draftRef.current = null;
    if (locked || next === null || next === selectedIndex || !stops[next]) {
      setDraft(null);
      return;
    }
    try {
      await onSelect(stops[next].id);
    } finally {
      setDraft(null);
    }
  };

  return (
    <div
      className="openbitfun-reasoning-slider"
      data-testid="chat-model-selector-intensity-slider"
      data-openbitfun-component="model-selector"
      data-openbitfun-part="reasoningSlider"
      data-disabled={disabled || (selectedIndex >= 0 && locked) ? 'true' : undefined}
      style={{ '--_reasoning-slider-progress': Math.max(0, value) / 4 } as React.CSSProperties}
    >
      <div className="openbitfun-reasoning-slider__header">
        {headerStart ?? (title && <span className="openbitfun-reasoning-slider__title">{title}</span>)}
        <div
          className="openbitfun-reasoning-slider__value"
          data-openbitfun-component="model-selector"
          data-openbitfun-part="reasoningSliderValue"
        >
          {renderValue ? renderValue(label) : label}
        </div>
      </div>
      <div className="openbitfun-reasoning-slider__control">
        <span className="openbitfun-reasoning-slider__track" aria-hidden="true">
          {selectedIndex >= 0 && <span className="openbitfun-reasoning-slider__fill" />}
        </span>
        {selectedIndex >= 0 && <input
          data-openbitfun-component="model-selector"
          data-openbitfun-part="reasoningSliderInput"
          type="range"
          min={0}
          max={4}
          step={1}
          value={value}
          disabled={locked}
          aria-label={t('reasoningSelector.title')}
          aria-valuetext={label}
          data-openbitfun-menu-item=""
          className="openbitfun-reasoning-slider__input"
          onChange={event => {
            preview(event.currentTarget.valueAsNumber);
            // Assistive controls may change a range without a pointer gesture.
            if (!pointerActiveRef.current) void commit();
          }}
          onPointerDown={event => {
            pointerActiveRef.current = true;
            event.currentTarget.setPointerCapture?.(event.pointerId);
          }}
          onPointerUp={() => {
            pointerActiveRef.current = false;
            void commit();
          }}
          onPointerCancel={() => {
            pointerActiveRef.current = false;
            cancel();
          }}
          onBlur={() => {
            pointerActiveRef.current = false;
            void commit();
          }}
          onKeyDown={event => {
            if (event.key === 'Escape') {
              cancel();
              return;
            }
            if (!adjustmentKeys.has(event.key)) return;
            event.preventDefault();
            event.stopPropagation();
            const current = draftRef.current ?? selectedIndex;
            const next = event.key === 'Home' ? supported[0]
              : event.key === 'End' ? supported[supported.length - 1]
                : event.key === 'ArrowRight' || event.key === 'ArrowUp'
                  ? supported.find(index => index > current) ?? current
                  : [...supported].reverse().find(index => index < current) ?? current;
            preview(next);
          }}
          onKeyUp={event => {
            if (!adjustmentKeys.has(event.key)) return;
            event.stopPropagation();
            void commit();
          }}
        />}
      </div>
    </div>
  );
};
