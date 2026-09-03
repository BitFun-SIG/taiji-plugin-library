import { Alert, Button, Combobox, ConfirmDialog, Input, NumberInput, Select, Switch, Tooltip, type ComboboxOption } from '@bitfun/ui';
import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Archive, FolderOpen } from 'lucide-react';
import { ConfigLoadingState, ConfigMessage, ConfigRetryState } from '@/infrastructure/config/components/common';
import { configAPI, workspaceAPI } from '@/infrastructure/api';
import { systemAPI } from '@/infrastructure/api/service-api/SystemAPI';
import type { CloseBehavior } from '@/infrastructure/api/service-api/SystemAPI';
import {
  getTerminalService,
  refreshTerminalPanelPosition,
  setTerminalPanelPosition,
} from '@/tools/terminal/services';
import type { ShellInfo } from '@/tools/terminal/types/session';
import {
  ConfigPageContent,
  ConfigPageHeader,
  ConfigPageLayout,
  ConfigPageSection,
  ConfigPageRow,
} from './common';
import { configManager } from '../services/ConfigManager';
import { createLogger } from '@/shared/utils/logger';
import type {
  BackendLogLevel,
  RuntimeLoggingInfo,
  TerminalConfig as TerminalSettings,
  TerminalPanelPosition,
} from '../types';
import './ApplicationSettingsPages.scss';

const log = createLogger('ApplicationSettings');

type TerminalShellOption = ComboboxOption & {
  shell?: ShellInfo;
};

const formatShellLabel = (shell: ShellInfo): string =>
  `${shell.name}${shell.version ? ` (${shell.version})` : ''}`;

function LaunchAtLoginSetting() {
  const { t } = useTranslation('settings/application');
  const isTauri = typeof window !== 'undefined' && '__TAURI__' in window;
  const [enabled, setEnabled] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ type: 'success' | 'error' | 'info'; text: string } | null>(null);

  const showMessage = useCallback((type: 'success' | 'error' | 'info', text: string) => {
    setMessage({ type, text });
    setTimeout(() => setMessage(null), 3000);
  }, []);

  const loadData = useCallback(async () => {
    if (!isTauri) return;
    setLoading(true);
    setLoadFailed(false);
    try {
      const value = await systemAPI.getLaunchAtLoginEnabled();
      setEnabled(value);
    } catch (error) {
      log.error('Failed to load launch-at-login state', error);
      setLoadFailed(true);
    } finally {
      setLoading(false);
    }
  }, [isTauri]);

  useEffect(() => {
    if (!isTauri) {
      setLoading(false);
      return;
    }
    void loadData().catch(() => undefined);
  }, [isTauri, loadData]);

  const handleToggle = useCallback(
    async (next: boolean) => {
      const previous = enabled;
      setEnabled(next);
      setSaving(true);
      try {
        await systemAPI.setLaunchAtLoginEnabled(next);
      } catch (error) {
        setEnabled(previous);
        log.error('Failed to set launch-at-login', { next, error });
        showMessage('error', t('launchAtLogin.messages.saveFailed'));
      } finally {
        setSaving(false);
      }
    },
    [enabled, showMessage, t]
  );

  if (!isTauri) {
    return null;
  }

  if (loading) {
    return <ConfigLoadingState label={t('launchAtLogin.messages.loading')} />;
  }

  if (loadFailed) {
    return (
      <ConfigRetryState
        message={t('launchAtLogin.messages.loadFailed')}
        retryLabel={t('common.retry')}
        onRetry={() => void loadData()}
      />
    );
  }

  return (
    <>
      <ConfigMessage message={message} />
      <ConfigPageRow
        label={t('launchAtLogin.toggleLabel')}
        description={t('launchAtLogin.toggleDescription')}
        align="center"
      >
        <div data-bf-component="application-settings" data-bf-part="launchAtLogin">
          <Switch
            checked={enabled}
            onChange={(e) => {
              void handleToggle(e.target.checked);
            }}
            disabled={saving}
          />
        </div>
      </ConfigPageRow>
    </>
  );
}

function AutoUpdateSetting() {
  const { t } = useTranslation('settings/application');
  const isTauri = typeof window !== 'undefined' && '__TAURI__' in window;
  const [enabled, setEnabled] = useState(true);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ type: 'success' | 'error' | 'info'; text: string } | null>(null);

  const showMessage = useCallback((type: 'success' | 'error' | 'info', text: string) => {
    setMessage({ type, text });
    setTimeout(() => setMessage(null), 3000);
  }, []);

  const loadData = useCallback(async () => {
    if (!isTauri) return;
    setLoading(true);
    setLoadFailed(false);
    try {
      const value = await configManager.getOptionalConfig<boolean>('app.auto_update');
      setEnabled(value !== false);
    } catch (error) {
      log.error('Failed to load app.auto_update', error);
      setLoadFailed(true);
    } finally {
      setLoading(false);
    }
  }, [isTauri]);

  useEffect(() => {
    if (!isTauri) {
      setLoading(false);
      return;
    }
    void loadData();
  }, [isTauri, loadData]);

  const handleToggle = useCallback(
    async (next: boolean) => {
      const previous = enabled;
      setEnabled(next);
      setSaving(true);
      try {
        await configManager.setConfig('app.auto_update', next);
        configManager.clearCache();
        showMessage('success', t('autoUpdate.messages.saved'));
      } catch (error) {
        setEnabled(previous);
        log.error('Failed to set app.auto_update', { next, error });
        showMessage('error', t('autoUpdate.messages.saveFailed'));
      } finally {
        setSaving(false);
      }
    },
    [enabled, showMessage, t]
  );

  if (!isTauri) {
    return null;
  }

  if (loading) {
    return <ConfigLoadingState label={t('autoUpdate.messages.loading')} />;
  }

  if (loadFailed) {
    return (
      <ConfigRetryState
        message={t('autoUpdate.messages.loadFailed')}
        retryLabel={t('common.retry')}
        onRetry={() => void loadData()}
      />
    );
  }

  return (
    <>
      <ConfigMessage message={message} />
      <ConfigPageRow
        label={t('autoUpdate.toggleLabel')}
        description={t('autoUpdate.toggleDescription')}
        align="center"
      >
        <div data-bf-component="application-settings" data-bf-part="autoUpdate">
          <Switch
            checked={enabled}
            onChange={(e) => {
              void handleToggle(e.target.checked);
            }}
            disabled={saving}
          />
        </div>
      </ConfigPageRow>
    </>
  );
}

function PreventSleepSetting() {
  const { t } = useTranslation('settings/application');
  const isTauri = typeof window !== 'undefined' && '__TAURI__' in window;
  const [enabled, setEnabled] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ type: 'success' | 'error' | 'info'; text: string } | null>(null);

  const showMessage = useCallback((type: 'success' | 'error' | 'info', text: string) => {
    setMessage({ type, text });
    setTimeout(() => setMessage(null), 3000);
  }, []);

  const loadData = useCallback(async () => {
    if (!isTauri) return;
    setLoading(true);
    setLoadFailed(false);
    try {
      setEnabled(await systemAPI.getPreventSleepEnabled());
    } catch (error) {
      log.error('Failed to load prevent-sleep preference', error);
      setLoadFailed(true);
    } finally {
      setLoading(false);
    }
  }, [isTauri]);

  useEffect(() => {
    if (!isTauri) {
      setLoading(false);
      return;
    }
    void loadData();
  }, [isTauri, loadData]);

  const handleToggle = useCallback(
    async (next: boolean) => {
      const previous = enabled;
      setEnabled(next);
      setSaving(true);
      try {
        await systemAPI.setPreventSleepEnabled(next);
        showMessage('success', t('preventSleep.messages.saved'));
      } catch (error) {
        setEnabled(previous);
        log.error('Failed to set prevent-sleep preference', { next, error });
        showMessage('error', t('preventSleep.messages.saveFailed'));
      } finally {
        setSaving(false);
      }
    },
    [enabled, showMessage, t]
  );

  if (!isTauri) {
    return null;
  }

  if (loading) {
    return <ConfigLoadingState label={t('preventSleep.messages.loading')} />;
  }

  if (loadFailed) {
    return (
      <ConfigRetryState
        message={t('preventSleep.messages.loadFailed')}
        retryLabel={t('common.retry')}
        onRetry={() => void loadData()}
      />
    );
  }

  return (
    <>
      <ConfigMessage message={message} />
      <ConfigPageRow
        label={t('preventSleep.toggleLabel')}
        description={t('preventSleep.toggleDescription')}
        align="center"
      >
        <div data-bf-component="application-settings" data-bf-part="preventSleep">
          <Switch
            checked={enabled}
            onChange={(event) => {
              void handleToggle(event.target.checked);
            }}
            disabled={saving}
          />
        </div>
      </ConfigPageRow>
    </>
  );
}

function LoggingSection() {
  const { t } = useTranslation('settings/application');
  const [configLevel, setConfigLevel] = useState<BackendLogLevel>('info');
  const [includeSensitiveDiagnostics, setIncludeSensitiveDiagnostics] = useState(false);
  const [runtimeInfo, setRuntimeInfo] = useState<RuntimeLoggingInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [openingFolder, setOpeningFolder] = useState(false);
  const [exportingDiagnostics, setExportingDiagnostics] = useState(false);
  const [exportConfirmOpen, setExportConfirmOpen] = useState(false);
  const [message, setMessage] = useState<{ type: 'success' | 'error' | 'info'; text: string } | null>(null);

  const levelOptions = useMemo(
    () => [
      { value: 'trace', label: t('logging.levels.trace') },
      { value: 'debug', label: t('logging.levels.debug') },
      { value: 'info', label: t('logging.levels.info') },
      { value: 'warn', label: t('logging.levels.warn') },
      { value: 'error', label: t('logging.levels.error') },
      { value: 'off', label: t('logging.levels.off') },
    ],
    [t]
  );

  const showMessage = useCallback((type: 'success' | 'error' | 'info', text: string) => {
    setMessage({ type, text });
    setTimeout(() => setMessage(null), 3000);
  }, []);

  const loadData = useCallback(async () => {
    try {
      setLoading(true);
      setLoadFailed(false);

      const [savedLevel, savedIncludeSensitiveDiagnostics, info] = await Promise.all([
        configManager.getConfig<BackendLogLevel>('app.logging.level'),
        configManager.getConfig<boolean>('app.logging.include_sensitive_diagnostics'),
        configAPI.getRuntimeLoggingInfo(),
      ]);

      setConfigLevel(savedLevel || info.effectiveLevel || 'info');
      setIncludeSensitiveDiagnostics(savedIncludeSensitiveDiagnostics ?? false);
      setRuntimeInfo(info);
    } catch (error) {
      log.error('Failed to load logging config', error);
      setLoadFailed(true);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadData();
  }, [loadData]);

  const handleLevelChange = useCallback(
    async (value: string) => {
      const nextLevel = value as BackendLogLevel;
      const previousLevel = configLevel;
      setConfigLevel(nextLevel);
      setSaving(true);

      try {
        await configManager.setConfig('app.logging.level', nextLevel);
        configManager.clearCache();

        const info = await configAPI.getRuntimeLoggingInfo();
        setRuntimeInfo(info);
        showMessage('success', t('logging.messages.levelUpdated'));
      } catch (error) {
        setConfigLevel(previousLevel);
        log.error('Failed to update logging level', { nextLevel, error });
        showMessage('error', t('logging.messages.saveFailed'));
      } finally {
        setSaving(false);
      }
    },
    [configLevel, showMessage, t]
  );

  const handleSensitiveDiagnosticsChange = useCallback(
    async (checked: boolean) => {
      const previousValue = includeSensitiveDiagnostics;
      setIncludeSensitiveDiagnostics(checked);
      setSaving(true);

      try {
        await configManager.setConfig('app.logging.include_sensitive_diagnostics', checked);
        configManager.clearCache();
        showMessage('success', t('logging.messages.sensitiveDiagnosticsUpdated'));
      } catch (error) {
        setIncludeSensitiveDiagnostics(previousValue);
        log.error('Failed to update sensitive diagnostics logging preference', { checked, error });
        showMessage('error', t('logging.messages.saveFailed'));
      } finally {
        setSaving(false);
      }
    },
    [includeSensitiveDiagnostics, showMessage, t]
  );

  const handleOpenFolder = useCallback(async () => {
    const folder = runtimeInfo?.sessionLogDir;
    if (!folder) {
      showMessage('error', t('logging.messages.pathUnavailable'));
      return;
    }

    try {
      setOpeningFolder(true);
      await workspaceAPI.revealInExplorer(folder);
    } catch (error) {
      log.error('Failed to open log folder', { folder, error });
      showMessage('error', t('logging.messages.openFailed'));
    } finally {
      setOpeningFolder(false);
    }
  }, [runtimeInfo?.sessionLogDir, showMessage, t]);

  const handleExportDiagnostics = useCallback(async () => {
    setExportConfirmOpen(false);
    try {
      setExportingDiagnostics(true);
      const result = await configAPI.exportDiagnosticsBundle();
      showMessage('success', t('logging.messages.diagnosticsExported'));
      await workspaceAPI.revealInExplorer(result.bundlePath);
    } catch (error) {
      log.error('Failed to export diagnostics bundle', { error });
      showMessage('error', t('logging.messages.diagnosticsExportFailed'));
    } finally {
      setExportingDiagnostics(false);
    }
  }, [showMessage, t]);

  if (loading) {
    return <ConfigLoadingState label={t('logging.messages.loading')} />;
  }

  if (loadFailed) {
    return (
      <ConfigRetryState
        message={t('logging.messages.loadFailed')}
        retryLabel={t('common.retry')}
        onRetry={() => void loadData()}
      />
    );
  }

  return (
    <div className="bitfun-logging-config" data-bf-component="application-settings" data-bf-part="logging">
      <div className="bitfun-logging-config__content">
        <ConfigMessage message={message} />

        {runtimeInfo?.previousUnexpectedExit?.detected && (
          <Alert
            tone={runtimeInfo.previousUnexpectedExit.category === 'crash' ? 'warning' : 'info'}
            message={t(
              runtimeInfo.previousUnexpectedExit.category === 'crash'
                ? 'logging.previousCrash.title'
                : 'logging.previousUncleanShutdown.title'
            )}
            description={t(
              runtimeInfo.previousUnexpectedExit.category === 'crash'
                ? 'logging.previousCrash.description'
                : 'logging.previousUncleanShutdown.description',
              {
                path: runtimeInfo.previousUnexpectedExit.sessionLogDir || '-',
              }
            )}
          />
        )}

        <ConfigPageSection
          title={t('logging.sections.logging')}
          description={t('logging.sections.loggingHint')}
        >
          <ConfigPageRow
            label={t('logging.sections.level')}
            description={t('logging.level.description')}
            align="center"
          >
            <Select
              value={configLevel}
              onValueChange={(v) => handleLevelChange(v as string)}
              options={levelOptions}
              disabled={saving}
            />
          </ConfigPageRow>
          <ConfigPageRow
            label={t('logging.sensitiveDiagnostics.label')}
            description={t('logging.sensitiveDiagnostics.description')}
            align="center"
          >
            <Switch
              checked={includeSensitiveDiagnostics}
              onChange={(e) => {
                void handleSensitiveDiagnosticsChange(e.target.checked);
              }}
              disabled={saving}
            />
          </ConfigPageRow>
          <ConfigPageRow
            label={t('logging.sections.path')}
            description={t('logging.path.description')}
            multiline
          >
            <div className="bitfun-logging-config__path-row" data-bf-component="application-settings" data-bf-part="logPath">
              <div className="bitfun-logging-config__path-box">
                {runtimeInfo?.sessionLogDir || '-'}
              </div>
              <Tooltip content={t('logging.actions.openFolderTooltip')} placement="top">
                <button
                  type="button"
                  className="bitfun-logging-config__open-btn"
                  onClick={handleOpenFolder}
                  disabled={openingFolder || !runtimeInfo?.sessionLogDir}
                >
                  <FolderOpen size={14} />
                </button>
              </Tooltip>
            </div>
          </ConfigPageRow>
          <ConfigPageRow
            label={t('logging.diagnostics.label')}
            description={t('logging.diagnostics.description')}
            align="center"
          >
            <Button
              type="button"
              variant="outline"
              size="md"
              leadingIcon={<Archive />}
              data-testid="diagnostics-export-button"
              onClick={() => {
                setExportConfirmOpen(true);
              }}
              loading={exportingDiagnostics}
              disabled={exportingDiagnostics}
            >
              {t('logging.actions.exportDiagnostics')}
            </Button>
          </ConfigPageRow>
        </ConfigPageSection>
        <ConfirmDialog
          open={exportConfirmOpen}
          onOpenChange={() => setExportConfirmOpen(false)}
          onConfirm={() => void handleExportDiagnostics()}
          title={t('logging.diagnostics.confirmTitle')}
          message={t(includeSensitiveDiagnostics
            ? 'logging.diagnostics.confirmSensitive'
            : 'logging.diagnostics.confirmStandard')}
          confirmText={t('logging.diagnostics.confirmAction')}
          type={includeSensitiveDiagnostics ? 'warning' : 'info'}
        />
      </div>
    </div>
  );
}

function TerminalSection() {
  const { t } = useTranslation('settings/application');
  const [defaultShell, setDefaultShell] = useState<string>('');
  const [terminalPanelPosition, setTerminalPanelPositionState] = useState<TerminalPanelPosition>('right');
  const [availableShells, setAvailableShells] = useState<ShellInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ type: 'success' | 'error' | 'info'; text: string } | null>(null);

  const showMessage = useCallback((type: 'success' | 'error' | 'info', text: string) => {
    setMessage({ type, text });
    setTimeout(() => setMessage(null), 3000);
  }, []);

  const loadData = useCallback(async () => {
    try {
      setLoading(true);
      setLoadFailed(false);

      const [terminalConfig, shells] = await Promise.all([
        configManager.getConfig<TerminalSettings>('terminal'),
        getTerminalService().getAvailableShells(),
      ]);

      setDefaultShell(terminalConfig?.default_shell || '');
      setTerminalPanelPositionState(terminalConfig?.terminal_panel_position === 'bottom' ? 'bottom' : 'right');
      void refreshTerminalPanelPosition();

      const availableOnly = shells.filter((s) => s.available);
      setAvailableShells(availableOnly);
    } catch (error) {
      log.error('Failed to load terminal config data', error);
      setLoadFailed(true);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadData();
  }, [loadData]);

  const handleShellChange = useCallback(
    async (value: string) => {
      const previous = defaultShell;
      try {
        setSaving(true);
        setDefaultShell(value);

        await configManager.setConfig('terminal.default_shell', value);

        configManager.clearCache();

        showMessage('success', t('terminal.messages.updated'));
      } catch (error) {
        setDefaultShell(previous);
        log.error('Failed to save terminal config', { shell: value, error });
        showMessage('error', t('terminal.messages.saveFailed'));
      } finally {
        setSaving(false);
      }
    },
    [defaultShell, showMessage, t]
  );

  const handleTerminalPanelPositionChange = useCallback(
    async (value: TerminalPanelPosition) => {
      const previous = terminalPanelPosition;
      try {
        setSaving(true);
        setTerminalPanelPositionState(value);

        await setTerminalPanelPosition(value);
        configManager.clearCache();

        showMessage('success', t('terminal.messages.panelPositionUpdated'));
      } catch (error) {
        setTerminalPanelPositionState(previous);
        log.error('Failed to save terminal panel position', { value, error });
        showMessage('error', t('terminal.messages.saveFailed'));
      } finally {
        setSaving(false);
      }
    },
    [showMessage, t, terminalPanelPosition],
  );

  const shellOptions = useMemo<TerminalShellOption[]>(
    () => [
      { value: '', label: t('terminal.controls.autoDetect') },
      ...availableShells.map((shell) => ({
        description: shell.path,
        value: shell.path,
        label: formatShellLabel(shell),
        shell,
      })),
    ],
    [availableShells, t],
  );

  const selectedShell = useMemo(
    () =>
      availableShells.find((shell) => shell.path === defaultShell) ??
      availableShells.find((shell) => shell.shellType === defaultShell),
    [availableShells, defaultShell],
  );
  const selectedShellValue = selectedShell?.path ?? defaultShell;

  const terminalPanelPositionOptions = useMemo(
    () => [
      { value: 'right', label: t('terminal.panelPosition.options.right') },
      { value: 'bottom', label: t('terminal.panelPosition.options.bottom') },
    ],
    [t],
  );
  const shouldShowCmdFallbackNotice = selectedShell?.shellType === 'Cmd' || defaultShell === 'Cmd';

  if (loading) {
    return <ConfigLoadingState label={t('terminal.messages.loading')} />;
  }

  if (loadFailed) {
    return (
      <ConfigRetryState
        message={t('terminal.messages.loadFailed')}
        retryLabel={t('common.retry')}
        onRetry={() => void loadData()}
      />
    );
  }

  return (
    <div className="bitfun-terminal-config" data-bf-component="application-settings" data-bf-part="terminal">
      <div className="bitfun-terminal-config__content">
        <ConfigMessage message={message} />

        <ConfigPageSection
          title={t('terminal.sections.terminal')}
          description={t('terminal.sections.terminalHint')}
        >
          {shouldShowCmdFallbackNotice && (
            <Alert
              tone="info"
              message={t('terminal.controls.cmdFallbackMessage')}
            />
          )}
          <ConfigPageRow
            label={t('terminal.sections.defaultTerminal')}
            description={t('terminal.controls.description')}
            align="center"
          >
            {availableShells.length > 0 ? (
              <Combobox
                value={selectedShellValue}
                onValueChange={(v) => handleShellChange(v as string)}
                options={shellOptions}
                placeholder={t('terminal.controls.placeholder')}
                disabled={saving}
              />
            ) : (
              <div className="bitfun-terminal-config__no-shells">{t('terminal.controls.noShells')}</div>
            )}
          </ConfigPageRow>

          <ConfigPageRow
            label={t('terminal.panelPosition.label')}
            description={t('terminal.panelPosition.description')}
            align="center"
          >
            <Select
              value={terminalPanelPosition}
              onValueChange={(v) => handleTerminalPanelPositionChange(v as TerminalPanelPosition)}
              options={terminalPanelPositionOptions}
              placeholder={t('terminal.panelPosition.placeholder')}
              disabled={saving}
            />
          </ConfigPageRow>
        </ConfigPageSection>
      </div>
    </div>
  );
}

function WindowBehaviorSetting() {
  const { t } = useTranslation('settings/application');
  const isTauri = typeof window !== 'undefined' && '__TAURI__' in window;
  const [behavior, setBehavior] = useState<CloseBehavior>('quit');
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ type: 'success' | 'error' | 'info'; text: string } | null>(null);

  const showMessage = useCallback((type: 'success' | 'error' | 'info', text: string) => {
    setMessage({ type, text });
    setTimeout(() => setMessage(null), 3000);
  }, []);

  const behaviorOptions = useMemo(
    () => [
      { value: 'quit', label: t('windowBehavior.options.quit') },
      { value: 'minimize_to_tray', label: t('windowBehavior.options.minimizeToTray') },
      { value: 'ask', label: t('windowBehavior.options.ask') },
    ],
    [t]
  );

  const loadData = useCallback(async () => {
    if (!isTauri) return;
    setLoading(true);
    setLoadFailed(false);
    try {
      const value = await configManager.getOptionalConfig<CloseBehavior>('app.close_button_behavior');
      setBehavior(value ?? 'minimize_to_tray');
    } catch (error) {
      log.error('Failed to load close behavior', error);
      setLoadFailed(true);
    } finally {
      setLoading(false);
    }
  }, [isTauri]);

  useEffect(() => {
    if (!isTauri) {
      setLoading(false);
      return;
    }
    void loadData();
  }, [isTauri, loadData]);

  const handleChange = useCallback(
    async (value: string) => {
      const previous = behavior;
      const next = value as CloseBehavior;
      setBehavior(next);
      setSaving(true);
      try {
        await configManager.setConfig('app.close_button_behavior', next);
        configManager.clearCache();
        showMessage('success', t('windowBehavior.messages.saved'));
      } catch (error) {
        setBehavior(previous);
        log.error('Failed to save close behavior', { next, error });
        showMessage('error', t('windowBehavior.messages.saveFailed'));
      } finally {
        setSaving(false);
      }
    },
    [behavior, showMessage, t]
  );

  if (!isTauri) return null;

  if (loading) {
    return <ConfigLoadingState label={t('windowBehavior.messages.loading')} />;
  }

  if (loadFailed) {
    return (
      <ConfigRetryState
        message={t('windowBehavior.messages.loadFailed')}
        retryLabel={t('common.retry')}
        onRetry={() => void loadData()}
      />
    );
  }

  return (
    <>
      <ConfigMessage message={message} />
      <ConfigPageRow
        label={t('windowBehavior.closeButtonLabel')}
        description={t('windowBehavior.closeButtonDescription')}
        align="center"
      >
        <div data-bf-component="application-settings" data-bf-part="windowBehavior">
          <Select
            value={behavior}
            onValueChange={(v) => { void handleChange(v as string); }}
            options={behaviorOptions}
            disabled={saving}
          />
        </div>
      </ConfigPageRow>
    </>
  );
}

function NotificationSettings() {
  const { t } = useTranslation('settings/application');
  const [dialogNotify, setDialogNotify] = useState(true);
  const [permissionRequestNotify, setPermissionRequestNotify] = useState(true);
  const [startupTips, setStartupTips] = useState(true);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ type: 'success' | 'error'; text: string } | null>(null);

  const loadData = useCallback(async () => {
    setLoading(true);
    setLoadFailed(false);
    try {
      const [notify, permissionNotify, tips] = await Promise.all([
        configManager.getOptionalConfig<boolean>('app.notifications.dialog_completion_notify'),
        configManager.getOptionalConfig<boolean>('app.notifications.permission_request_notify'),
        configManager.getOptionalConfig<boolean>('app.notifications.enable_startup_tips'),
      ]);
      setDialogNotify(notify !== false);
      setPermissionRequestNotify(permissionNotify !== false);
      setStartupTips(tips !== false);
    } catch (error) {
      log.error('Failed to load notification preferences', error);
      setLoadFailed(true);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadData();
  }, [loadData]);

  const handleDialogNotifyToggle = async (checked: boolean) => {
    setSaving(true);
    try {
      await configAPI.setConfig('app.notifications.dialog_completion_notify', checked);
      setDialogNotify(checked);
      setMessage({ type: 'success', text: t('notifications.messages.saveSuccess') });
    } catch {
      setMessage({ type: 'error', text: t('notifications.messages.saveFailed') });
    } finally {
      setSaving(false);
    }
  };

  const handlePermissionRequestNotifyToggle = async (checked: boolean) => {
    setSaving(true);
    try {
      await configManager.setConfig('app.notifications.permission_request_notify', checked);
      setPermissionRequestNotify(checked);
      setMessage({ type: 'success', text: t('notifications.messages.saveSuccess') });
    } catch {
      setMessage({ type: 'error', text: t('notifications.messages.saveFailed') });
    } finally {
      setSaving(false);
    }
  };

  const handleStartupTipsToggle = async (checked: boolean) => {
    setSaving(true);
    try {
      await configAPI.setConfig('app.notifications.enable_startup_tips', checked);
      setStartupTips(checked);
      setMessage({ type: 'success', text: t('notifications.messages.saveSuccess') });
    } catch {
      setMessage({ type: 'error', text: t('notifications.messages.saveFailed') });
    } finally {
      setSaving(false);
    }
  };

  if (loading) {
    return <ConfigLoadingState label={t('notifications.messages.loading')} />;
  }

  if (loadFailed) {
    return (
      <ConfigRetryState
        message={t('notifications.messages.loadFailed')}
        retryLabel={t('common.retry')}
        onRetry={() => void loadData()}
      />
    );
  }

  return (
    <>
      <ConfigMessage message={message} />
      <ConfigPageRow
        label={t('notifications.dialogCompletion.label')}
        description={t('notifications.dialogCompletion.description')}
        align="center"
      >
        <div data-bf-component="application-settings" data-bf-part="notifications">
          <Switch
            checked={dialogNotify}
            onChange={(e) => { void handleDialogNotifyToggle(e.target.checked); }}
            disabled={saving}
          />
        </div>
      </ConfigPageRow>
      <ConfigPageRow
        label={t('notifications.permissionRequest.label')}
        description={t('notifications.permissionRequest.description')}
        align="center"
      >
        <Switch
          checked={permissionRequestNotify}
          onChange={(e) => { void handlePermissionRequestNotifyToggle(e.target.checked); }}
          disabled={saving}
        />
      </ConfigPageRow>
      <ConfigPageRow
        label={t('notifications.startupTips.label')}
        description={t('notifications.startupTips.description')}
        align="center"
      >
        <Switch
          checked={startupTips}
          onChange={(e) => { void handleStartupTipsToggle(e.target.checked); }}
          disabled={saving}
        />
      </ConfigPageRow>
    </>
  );
}

/**
 * Knowledge base root directory (UX-P1-3).
 *
 * Front-end entry for `ai.knowledge_base_root`. The desktop and CLI hosts
 * inject this value into the `BITFUN_KNOWLEDGE_BASE_ROOT` environment
 * variable at startup so the KnowledgeBaseSearch tool can resolve its root at
 * call time (L6-P0-1). Saving writes the config key directly; the next host
 * startup picks it up.
 */
function KnowledgeBaseSection() {
  const { t } = useTranslation('settings/application');
  const [root, setRoot] = useState('');
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ type: 'success' | 'error' | 'info'; text: string } | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        setLoading(true);
        const value = await configManager.getConfig<string>('ai.knowledge_base_root');
        if (!cancelled) {
          setRoot(value ?? '');
        }
      } catch (error) {
        log.error('Failed to load knowledge base root config', error);
        if (!cancelled) {
          setMessage({ type: 'error', text: t('knowledgeBase.messages.loadFailed') });
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [t]);

  const handleSave = useCallback(async () => {
    setSaving(true);
    const previous = root;
    const next = root.trim();
    try {
      if (next.length === 0) {
        await configManager.setConfig('ai.knowledge_base_root', '');
        configManager.clearCache();
        setMessage({ type: 'info', text: t('knowledgeBase.messages.cleared') });
        return;
      }
      await configManager.setConfig('ai.knowledge_base_root', next);
      configManager.clearCache();
      setRoot(next);
      setMessage({ type: 'success', text: t('knowledgeBase.messages.saved') });
    } catch (error) {
      setRoot(previous);
      log.error('Failed to save knowledge base root', { root: next, error });
      setMessage({ type: 'error', text: t('knowledgeBase.messages.saveFailed') });
    } finally {
      setSaving(false);
    }
  }, [root, t]);

  if (loading) {
    return <ConfigLoadingState label={t('knowledgeBase.messages.loading')} />;
  }

  return (
    <div className="bitfun-knowledge-base-config" data-bf-component="application-settings" data-bf-part="knowledgeBase">
      <div className="bitfun-knowledge-base-config__content">
        <ConfigMessage message={message} />
        <ConfigPageSection
          title={t('knowledgeBase.sections.title')}
          description={t('knowledgeBase.sections.hint')}
        >
          <ConfigPageRow
            label={t('knowledgeBase.rootLabel')}
            description={t('knowledgeBase.rootDescription')}
            align="center"
          >
            <Input
              value={root}
              onChange={(e) => setRoot(e.target.value)}
              placeholder={t('knowledgeBase.rootPlaceholder')}
              size="sm"
              disabled={saving}
              data-testid="basics-knowledge-base-root"
              aria-label={t('knowledgeBase.rootLabel')}
            />
          </ConfigPageRow>
          <ConfigPageRow
            label={t('knowledgeBase.actions.saveLabel')}
            description={t('knowledgeBase.actions.saveDescription')}
            align="center"
          >
            <Button
              type="button"
              onClick={() => void handleSave()}
              isLoading={saving}
              disabled={saving}
              data-testid="basics-knowledge-base-save"
            >
              {t('knowledgeBase.actions.save')}
            </Button>
          </ConfigPageRow>
        </ConfigPageSection>
      </div>
    </div>
  );
}

/**
 * Legion deployment thresholds (configurable via the unified threshold settings).
 *
 * Front-end entry for `ai.legion_max_nodes` (per-topology node cap, default 20),
 * `ai.legion_max_total_nodes` (cross-deployment total cap, default 60) and
 * `ai.legion_deploy_frequency_per_hour` (deployments per creator per hour,
 * default 10, 0 = unlimited). Saving writes the config keys directly so the
 * LegionControl tool picks them up at the next call (hot, no restart needed).
 *
 * NOTE (UX-P1-1): these three keys are TOP-LEVEL `ai.legion_*` keys — they are
 * intentionally NOT part of the `ai.thresholds.*` subdomain (there is no
 * `ai.thresholds.legion.*`). Writing `ai.thresholds.legion_*`/`legion.*` is
 * silently ignored by the config service, so keep this section on the
 * `ai.legion_*` top-level keys.
 */
function LegionThresholdsSection() {
  const { t } = useTranslation('settings/application');
  const [maxNodes, setMaxNodes] = useState(20);
  const [maxTotalNodes, setMaxTotalNodes] = useState(60);
  const [frequencyPerHour, setFrequencyPerHour] = useState(10);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ type: 'success' | 'error'; text: string } | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        setLoading(true);
        const [nodes, total, frequency] = await Promise.all([
          configManager.getConfig<number>('ai.legion_max_nodes'),
          configManager.getConfig<number>('ai.legion_max_total_nodes'),
          configManager.getConfig<number>('ai.legion_deploy_frequency_per_hour'),
        ]);
        if (!cancelled) {
          setMaxNodes(nodes ?? 20);
          setMaxTotalNodes(total ?? 60);
          setFrequencyPerHour(frequency ?? 10);
        }
      } catch (error) {
        log.error('Failed to load legion threshold config', error);
        if (!cancelled) {
          setMessage({ type: 'error', text: t('legion.messages.loadFailed') });
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [t]);

  const persist = useCallback(async (path: string, value: number) => {
    try {
      await configManager.setConfig(path, value);
      configManager.clearCache();
      return true;
    } catch (error) {
      log.error(`Failed to save legion threshold ${path}`, { value, error });
      return false;
    }
  }, []);

  const handleMaxNodesChange = useCallback(async (value: number) => {
    setSaving(true);
    const previous = maxNodes;
    setMaxNodes(value);
    try {
      if (value < 1) {
        // A per-topology cap below 1 is meaningless; the backend clamps to the
        // default anyway. Surface it and restore the previous value.
        setMessage({ type: 'error', text: t('legion.messages.invalidNodeCap') });
        setMaxNodes(previous);
        return;
      }
      const ok = await persist('ai.legion_max_nodes', value);
      setMessage({ type: ok ? 'success' : 'error', text: ok ? t('legion.messages.saved') : t('legion.messages.saveFailed') });
    } finally {
      setSaving(false);
    }
  }, [maxNodes, persist, t]);

  const handleMaxTotalNodesChange = useCallback(async (value: number) => {
    setSaving(true);
    const previous = maxTotalNodes;
    setMaxTotalNodes(value);
    try {
      if (value < 1) {
        setMessage({ type: 'error', text: t('legion.messages.invalidTotalCap') });
        setMaxTotalNodes(previous);
        return;
      }
      const ok = await persist('ai.legion_max_total_nodes', value);
      setMessage({ type: ok ? 'success' : 'error', text: ok ? t('legion.messages.saved') : t('legion.messages.saveFailed') });
    } finally {
      setSaving(false);
    }
  }, [maxTotalNodes, persist, t]);

  const handleFrequencyChange = useCallback(async (value: number) => {
    setSaving(true);
    const previous = frequencyPerHour;
    setFrequencyPerHour(value);
    try {
      const ok = await persist('ai.legion_deploy_frequency_per_hour', value);
      setMessage({ type: ok ? 'success' : 'error', text: ok ? t('legion.messages.saved') : t('legion.messages.saveFailed') });
      if (!ok) setFrequencyPerHour(previous);
    } finally {
      setSaving(false);
    }
  }, [frequencyPerHour, persist, t]);

  if (loading) {
    return <ConfigLoadingState label={t('legion.messages.loading')} />;
  }

  return (
    <div className="bitfun-legion-thresholds-config" data-bf-component="application-settings" data-bf-part="legion">
      <div className="bitfun-legion-thresholds-config__content">
        <ConfigMessage message={message} />
        <ConfigPageSection
          title={t('legion.sections.title')}
          description={t('legion.sections.hint')}
        >
          <ConfigPageRow
            label={t('legion.maxNodes.label')}
            description={t('legion.maxNodes.description')}
            align="center"
          >
            <NumberInput
              value={maxNodes}
              onValueChange={(value) => void handleMaxNodesChange(value)}
              min={1}
              max={1000}
              step={1}
              size="sm"
              disabled={saving}
            />
          </ConfigPageRow>
          <ConfigPageRow
            label={t('legion.maxTotalNodes.label')}
            description={t('legion.maxTotalNodes.description')}
            align="center"
          >
            <NumberInput
              value={maxTotalNodes}
              onValueChange={(value) => void handleMaxTotalNodesChange(value)}
              min={1}
              max={10000}
              step={1}
              size="sm"
              disabled={saving}
            />
          </ConfigPageRow>
          <ConfigPageRow
            label={t('legion.frequency.label')}
            description={t('legion.frequency.description')}
            align="center"
          >
            <NumberInput
              value={frequencyPerHour}
              onValueChange={(value) => void handleFrequencyChange(value)}
              min={0}
              max={10000}
              step={1}
              size="sm"
              disabled={saving}
            />
          </ConfigPageRow>
        </ConfigPageSection>
      </div>
    </div>
  );
}

interface ApplicationSettingsPageProps {
  page: 'general' | 'terminal' | 'diagnostics';
}

const ApplicationSettingsPage: React.FC<ApplicationSettingsPageProps> = ({ page }) => {
  const { t } = useTranslation('settings');
  const { t: tApplication } = useTranslation('settings/application');
  const isTauri = typeof window !== 'undefined' && '__TAURI__' in window;

  const title = t(`navigation.pages.${page}.label`);
  const subtitle = t(`navigation.pages.${page}.description`);

  return (
    <ConfigPageLayout
      className="bitfun-application-settings"
      data-bf-component="application-settings"
      data-bf-part="root"
      data-bf-view={page}
    >
      <ConfigPageHeader title={title} subtitle={subtitle} />
      <ConfigPageContent
        className="bitfun-application-settings__content"
        data-bf-component="application-settings"
        data-bf-part="content"
      >
        {page === 'general' ? (
          <>
            {isTauri && (
              <ConfigPageSection
                title={tApplication('applicationGroups.startupAndUpdates.title')}
                description={tApplication('applicationGroups.startupAndUpdates.description')}
              >
                <LaunchAtLoginSetting />
                <PreventSleepSetting />
                <AutoUpdateSetting />
              </ConfigPageSection>
            )}
            <ConfigPageSection
              title={tApplication('applicationGroups.windowAndNotifications.title')}
              description={tApplication('applicationGroups.windowAndNotifications.description')}
            >
              <WindowBehaviorSetting />
              <NotificationSettings />
            </ConfigPageSection>
            <KnowledgeBaseSection />
            <LegionThresholdsSection />
          </>
        ) : null}
        {page === 'terminal' ? <TerminalSection /> : null}
        {page === 'diagnostics' ? <LoggingSection /> : null}
      </ConfigPageContent>
    </ConfigPageLayout>
  );
};

export function GeneralSettingsPage(): React.ReactElement {
  return <ApplicationSettingsPage page="general" />;
}

export function TerminalSettingsPage(): React.ReactElement {
  return <ApplicationSettingsPage page="terminal" />;
}

export function DiagnosticsSettingsPage(): React.ReactElement {
  return <ApplicationSettingsPage page="diagnostics" />;
}
