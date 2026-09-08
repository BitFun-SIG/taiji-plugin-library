import type { AppearanceThemeTokenName } from '../types';

// Old packages colored field hints through content.muted. Explicit field colors
// take precedence; missing values continue to come from the selected base theme.
export function withLegacyFieldTokens(
  tokens: Partial<Record<AppearanceThemeTokenName, string>> | undefined,
): Partial<Record<AppearanceThemeTokenName, string>> {
  const result = { ...tokens };
  const legacy = tokens?.['--openbitfun-color-content-muted'];
  if (result['--openbitfun-color-field-placeholder'] === undefined && legacy !== undefined) {
    result['--openbitfun-color-field-placeholder'] = legacy;
  }
  return result;
}
