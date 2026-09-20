import type { Session } from '../types/flow-chat';
import type { SessionMetadata } from '@/shared/types/session-history';
/**
 * Session list membership is the owning execution workspace ID (`workspaceId`),
 * never a path. `projectWorkspaceId` is only the persistence owner (the main
 * project for linked worktrees) and a legacy fallback for records created
 * before execution-workspace stamping; it must never widen membership to
 * sibling worktrees of the same project.
 */
export function sessionBelongsToWorkspaceNavRow(
  session: Pick<Session, 'workspaceId' | 'projectWorkspaceId'>,
  workspaceId?: string,
): boolean {
  if (!workspaceId) return false;
  return (session.workspaceId ?? session.projectWorkspaceId) === workspaceId;
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
