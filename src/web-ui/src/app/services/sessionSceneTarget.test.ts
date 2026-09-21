import { describe, expect, it } from 'vitest';
import type { Session } from '@/flow_chat/types/flow-chat';
import type { WorkspaceInfo } from '@/shared/types';
import { getSessionSceneTabId } from '../components/SceneBar/types';
import { resolveSessionActivationWorkspace, resolveSessionSceneTarget, resolveSessionSceneWorkspace } from './sessionSceneTarget';

const session = (overrides: Partial<Session>): Session => ({
  sessionId: 'session', title: 'Session', config: {}, dialogTurns: [],
  status: 'idle', createdAt: 1, lastActiveAt: 1, error: null, ...overrides,
});
const tabId = (value: Session, surfaceId = 'local', workspaces: WorkspaceInfo[] = []) =>
  getSessionSceneTabId(resolveSessionSceneTarget(value, workspaces, surfaceId));

describe('workspace session tab identity', () => {
  it('keeps an execution worktree under its owning project tab', () => {
    expect(tabId(session({ workspacePath: '/project/.worktrees/task', projectWorkspacePath: '/project' })))
      .toBe(tabId(session({ workspacePath: '/project' })));
  });

  it('reads legacy config-only metadata without requiring a workspace id', () => {
    expect(tabId(session({ config: { workspacePath: 'D:\\project\\' } })))
      .toBe(tabId(session({ workspacePath: 'D:/project' })));
  });

  it('scopes identical ids and roots to their device surface', () => {
    const value = session({ workspaceId: 'same', workspacePath: '/project' });
    expect(tabId(value, 'peer-a')).not.toBe(tabId(value, 'peer-b'));
  });

  it('distinguishes workspace IDs even when their paths are identical', () => {
    const remote = session({ workspaceId: 'remote-a', workspacePath: '/Project' });
    expect(tabId(remote)).not.toBe(tabId(session({ ...remote, workspaceId: 'remote-b' })));
    expect(tabId(remote)).not.toBe(tabId(session({ workspaceId: 'local', workspacePath: '/Project' })));
    expect(tabId(remote)).toBe(tabId(session({ ...remote, workspacePath: '/moved' })));
  });

  it('resolves old session metadata to the live workspace id', () => {
    const workspace = { id: 'workspace', rootPath: '/project' } as WorkspaceInfo;
    expect(tabId(session({ workspacePath: '/project' }), 'local', [workspace]))
      .toBe(tabId(session({ workspaceId: 'workspace', workspacePath: '/project' })));
  });

  it('never resolves a remote session to a local directory with the same path', () => {
    const local = { id: 'local-project', rootPath: '/project' } as WorkspaceInfo;
    const remote = session({ workspacePath: '/project', remoteSshHost: 'server' });
    expect(tabId(remote, 'local', [local])).toBe(tabId(remote));
    expect(tabId(remote, 'local', [local])).not.toBe(tabId(session({ workspaceId: local.id })));
  });
});

describe('session activation workspace identity', () => {
  const project = { id: 'project', rootPath: '/project' } as WorkspaceInfo;
  const worktree = { id: 'worktree', rootPath: '/worktrees/task' } as WorkspaceInfo;

  it('activates the worktree a session is listed under, not its persistence project', () => {
    const value = session({
      workspaceId: worktree.id, projectWorkspaceId: project.id,
      workspacePath: worktree.rootPath, projectWorkspacePath: project.rootPath,
    });

    expect(resolveSessionSceneWorkspace(value, [project, worktree])?.id).toBe(project.id);
    expect(resolveSessionActivationWorkspace(value, [project, worktree])?.id).toBe(worktree.id);
  });

  it('falls back to the project only for records that carry no workspace identity', () => {
    expect(resolveSessionActivationWorkspace(session({ workspacePath: '/project' }), [project])?.id).toBe(project.id);
    expect(resolveSessionActivationWorkspace(session({ projectWorkspaceId: project.id }), [project])?.id).toBe(project.id);
    expect(resolveSessionActivationWorkspace(session({ config: { workspaceId: worktree.id } }), [project, worktree])?.id)
      .toBe(worktree.id);
  });

  it('stays unresolved when the listed workspace is not open here', () => {
    expect(resolveSessionActivationWorkspace(session({ workspaceId: 'closed' }), [project, worktree])).toBeUndefined();
  });
});
