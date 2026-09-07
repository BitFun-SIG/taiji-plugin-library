import { createTauriCommandError } from '../errors/TauriCommandError';
import { api } from './ApiClient';

export type MigrationGroupId =
  | 'settings_and_credentials'
  | 'agents_skills_and_miniapps'
  | 'workspaces_sessions_and_tasks'
  | 'memory'
  | 'remote_connections_and_devices';

export type MigrationDomainId =
  | 'settings'
  | 'credentials'
  | 'skills'
  | 'miniapps'
  | 'agents'
  | 'workspace_sessions'
  | 'agent_coordination'
  | 'structured_memory'
  | 'file_memory'
  | 'remote_connect_devices'
  | 'remote_ssh'
  | 'cross_reference_repair';

export type MigrationPromptChoice =
  | 'unset'
  | 'migrate_now'
  | 'remind_later'
  | 'do_not_remind';

export type MigrationRunStatus =
  | 'discovered'
  | 'scanned'
  | 'planned'
  | 'waiting_for_processes'
  | 'staging'
  | 'validating_stage'
  | 'committing'
  | 'validating_commit'
  | 'completed'
  | 'completed_with_warnings'
  | 'cancelled'
  | 'failed_recoverable'
  | 'failed_manual_action_required';

export type MigrationDomainState =
  | 'not_started'
  | 'staged'
  | 'committed'
  | 'verified'
  | 'failed'
  | 'skipped';

export interface MigrationSelection {
  groups: MigrationGroupId[];
}

export interface MigrationDiagnostic {
  code: string;
  severity: 'info' | 'warning' | 'blocking';
  domain: MigrationDomainId | null;
  relativePath: string | null;
  message: string;
  action: string | null;
}

export interface LegacySourceDescriptor {
  sourceId: string;
  sourceFingerprint: string;
  productId: string;
  productVersion: string;
  platform: string;
  readable: boolean;
  supported: boolean;
  approximateBytes: number;
  alreadyMigrated: boolean;
  diagnostics: MigrationDiagnostic[];
}

export interface MigrationOnboardingState {
  formatVersion: number;
  sourceFingerprint: string;
  detectedAtMs: number | null;
  lastScannedAtMs: number | null;
  choice: MigrationPromptChoice;
  lastPromptedVersion: string | null;
  runId: string | null;
  lastReportRunId: string | null;
  handledRunId: string | null;
}

export interface MigrationDomainResult {
  domain: MigrationDomainId;
  state: MigrationDomainState;
  imported: number;
  skipped: number;
  conflicts: number;
  warnings: MigrationDiagnostic[];
  requiresReauthentication: string[];
  requiresRelocation: string[];
}

export interface MigrationRunReport {
  formatVersion: number;
  runId: string;
  sourceFingerprint: string;
  planHash: string;
  status: MigrationRunStatus;
  startedAtMs: number;
  finishedAtMs: number | null;
  domainResults: MigrationDomainResult[];
  diagnostics: MigrationDiagnostic[];
  requiresReauthentication: string[];
  requiresRelocation: string[];
}

export interface LegacyMigrationStatusView {
  source: LegacySourceDescriptor | null;
  onboarding: MigrationOnboardingState;
  latestReport: MigrationRunReport | null;
  startupReport: MigrationRunReport | null;
}

export interface ScanFinding {
  domain: MigrationDomainId;
  code: string;
  severity: 'info' | 'warning' | 'blocking';
  entityCount: number;
  logicalBytes: number;
  sourceSchema: string | null;
  migratable: boolean;
  detail: string;
}

export interface LegacyMigrationScanView {
  source: LegacySourceDescriptor;
  selection: MigrationSelection;
  scannedAtMs: number;
  findings: ScanFinding[];
}

export interface LegacyMigrationHandoffView {
  runId: string;
  mode: 'onboarding' | 'execute';
}

export class LegacyMigrationAPI {
  private async invoke<T>(command: string, request: object): Promise<T> {
    try {
      return await api.invoke<T>(command, { request });
    } catch (error) {
      throw createTauriCommandError(command, error, request);
    }
  }

  getStatus(): Promise<LegacyMigrationStatusView> {
    return this.invoke('get_legacy_migration_status', {});
  }

  scan(selection?: MigrationSelection): Promise<LegacyMigrationScanView> {
    return this.invoke('scan_legacy_migration', selection ? { selection } : {});
  }

  prepare(selection: MigrationSelection): Promise<LegacyMigrationHandoffView> {
    return this.invoke('prepare_legacy_migration', { selection });
  }

  getReport(runId?: string): Promise<MigrationRunReport | null> {
    return this.invoke('get_legacy_migration_report', runId ? { runId } : {});
  }

  setPromptPreference(choice: MigrationPromptChoice): Promise<MigrationOnboardingState> {
    return this.invoke('set_legacy_migration_prompt_preference', { choice });
  }
}

export const legacyMigrationAPI = new LegacyMigrationAPI();
