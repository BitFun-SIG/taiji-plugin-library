import type { Session } from '../types/flow-chat';
import type { SessionMetadata } from '@/shared/types/session-history';
/**
 * Session facts that decide which navigation group owns a session. `config` is
 * optional so metadata-only and legacy call sites keep working.
 */
type SessionNavigationOwner = Pick<Session, 'workspaceId' | 'projectWorkspaceId'> & {
  config?: Pick<Session['config'], 'executionTarget'>;
};

/**
 * The session executes in a managed worktree of its owning project.
 *
 * A worktree is an execution directory, not a project of its own, so this fact
 * is what keeps such a session in its project's navigation group.
 */
export function isWorktreeIsolatedSession(session: SessionNavigationOwner): boolean {
  const target = session.config?.executionTarget;
  return !!target && target.kind !== 'local';
}

/**
 * Session list membership is the owning project workspace ID, never a path.
 *
 * The backend stamps a worktree-isolated session with the worktree's own
 * workspace record, but that record is created on demand and is normally not an
 * open workspace. Following it would drop the session out of every navigation
 * group the user can see, so an isolated session stays under the project that
 * owns it — `projectWorkspaceId` is the identity the worktree cannot outlive.
 *
 * `projectWorkspaceId` is a legacy fallback for records created before
 * execution-workspace stamping, and it must still never widen membership to
 * sibling worktrees: only the session's own owning project matches.
 */
export function sessionBelongsToWorkspaceNavRow(
  session: SessionNavigationOwner,
  workspaceId?: string,
): boolean {
  if (!workspaceId) return false;
  const ownerWorkspaceId = isWorktreeIsolatedSession(session)
    ? (session.projectWorkspaceId ?? session.workspaceId)
    : (session.workspaceId ?? session.projectWorkspaceId);
  return ownerWorkspaceId === workspaceId;
}

export function getSessionSortTimestamp(session: Pick<Session, 'createdAt' | 'lastFinishedAt'>): number {
  return session.lastFinishedAt ?? session.createdAt;
}

export function compareSessionsForDisplay(
  a: Pick<Session, 'sessionId' | 'createdAt' | 'lastFinishedAt'>,
  b: Pick<Session, 'sessionId' | 'createdAt' | 'lastFinishedAt'>
): number {
  const timestampDiff = getSessionSortTimestamp(b) - getSessionSortTimestamp(a);
  if (timestampDiff !== 0) {
    return timestampDiff;
  }

  const createdAtDiff = b.createdAt - a.createdAt;
  if (createdAtDiff !== 0) {
    return createdAtDiff;
  }

  return a.sessionId.localeCompare(b.sessionId);
}

export function getSessionMetadataSortTimestamp(
  session: Pick<SessionMetadata, 'createdAt' | 'lastFinishedAt' | 'customMetadata'>
): number {
  const lastFinishedAt = session.lastFinishedAt ?? session.customMetadata?.lastFinishedAt;
  return typeof lastFinishedAt === 'number' ? lastFinishedAt : session.createdAt;
}

export function compareSessionMetadataForDisplay(
  a: Pick<SessionMetadata, 'sessionId' | 'createdAt' | 'lastFinishedAt' | 'customMetadata'>,
  b: Pick<SessionMetadata, 'sessionId' | 'createdAt' | 'lastFinishedAt' | 'customMetadata'>
): number {
  const timestampDiff = getSessionMetadataSortTimestamp(b) - getSessionMetadataSortTimestamp(a);
  if (timestampDiff !== 0) {
    return timestampDiff;
  }

  const createdAtDiff = b.createdAt - a.createdAt;
  if (createdAtDiff !== 0) {
    return createdAtDiff;
  }

  return a.sessionId.localeCompare(b.sessionId);
}

/**
 * Left-nav session list order: newest-created first, stable while switching sessions
 * (does not use `lastActiveAt`, so rows do not jump to the top on click).
 */
export function compareSessionsForNavStable(
  a: Pick<Session, 'sessionId' | 'createdAt'>,
  b: Pick<Session, 'sessionId' | 'createdAt'>
): number {
  const createdAtDiff = b.createdAt - a.createdAt;
  if (createdAtDiff !== 0) {
    return createdAtDiff;
  }

  return a.sessionId.localeCompare(b.sessionId);
}
