import { flowChatStore } from '@/flow_chat/store/FlowChatStore';
import { startAutoSync } from '@/flow_chat/services/storeSync';
import { workspaceManager } from '@/infrastructure/services/business/workspaceManager';
import { getActiveSurfaceId, isSurfaceChangedError } from '@/infrastructure/peer-device/deviceSurface';
import { createLogger } from '@/shared/utils/logger';
import { registerSessionSceneNavigation, useSceneStore } from '../stores/sceneStore';
import { isSessionSceneId, type SessionSceneTarget } from '../components/SceneBar/types';
import { resolveSessionSceneTarget, resolveSessionSceneWorkspace } from './sessionSceneTarget';

const log = createLogger('SessionSceneLifecycle');

/**
 * Tabs own one resource reference per workspace; FlowChat owns all sessions.
 * The shell coordinates navigation with the one active-session presentation.
 */
export function startSessionSceneLifecycle(): () => void {
  const stopProjectionSync = startAutoSync();
  const current = () => {
    const session = flowChatStore.getActiveSession();
    return session ? resolveSessionSceneTarget(
      session, workspaceManager.getState().openedWorkspaces.values(), getActiveSurfaceId(),
    ) : null;
  };
  const stopNavigation = registerSessionSceneNavigation({
    current,
    isActive: target => {
      if (target.surfaceId !== getActiveSurfaceId() || current()?.sessionId !== target.sessionId) return false;
      const session = flowChatStore.getActiveSession()!;
      const state = workspaceManager.getState();
      const workspace = resolveSessionSceneWorkspace(session, state.openedWorkspaces.values());
      return workspace ? workspace.id === state.activeWorkspaceId : !state.currentWorkspace;
    },
    activate: async (target, isCurrent) => {
      if (target.surfaceId !== getActiveSurfaceId() || !isCurrent()) return false;
      try {
        const { activateMainSession } = await import('@/flow_chat/services/sessionActivation');
        if (!isCurrent()) return false;
        return await activateMainSession(target.sessionId, { isCurrent });
      } catch (error) {
        if (isCurrent() && !isSurfaceChangedError(error)) {
          log.error('Failed to activate session tab', { sessionId: target.sessionId, error });
          const { notificationService } = await import('@/shared/notification-system');
          if (isCurrent()) notificationService.error(error instanceof Error ? error.message : String(error));
        }
        return false;
      }
    },
  });

  let previousSelection = current();
  let previousSurface = getActiveSurfaceId();
  let reconciling = false;
  const reconcile = () => {
    if (reconciling) return;
    reconciling = true;
    try {
      const surfaceId = getActiveSurfaceId();
      if (surfaceId !== previousSurface) {
        previousSurface = surfaceId;
        previousSelection = current();
        useSceneStore.getState().resetForPeerSwitch();
      }
      const selected = current();
      const selectionChanged = selected?.sessionId !== previousSelection?.sessionId
        || selected?.workspaceKey !== previousSelection?.workspaceKey;
      previousSelection = selected;
      const scenes = useSceneStore.getState();
      // The pending navigation owns the upcoming selection. Bootstrap/hydrate
      // notifications cannot replace its destination or steal its focus.
      if (selectionChanged && selected && !scenes.pendingTabId) {
        if (isSessionSceneId(scenes.activeTabId)) {
          scenes.openSessionScene(selected);
        } else {
          scenes.updateSessionScene(selected);
        }
      }
      const source = flowChatStore.getState();
      const targets = new Map<string, SessionSceneTarget>();
      for (const tab of useSceneStore.getState().openTabs) {
        const session = tab.session?.surfaceId === surfaceId
          ? source.sessions.get(tab.session.sessionId) : undefined;
        if (session) targets.set(session.sessionId, scenes.pendingTabId && tab.session
          // Keep the transaction's identity until it commits. Metadata may
          // acquire a workspace id while activation is awaiting the host.
          ? tab.session
          : resolveSessionSceneTarget(
            session, workspaceManager.getState().openedWorkspaces.values(), surfaceId,
          ));
      }
      useSceneStore.getState().reconcileSessionScenes(targets);
    } finally {
      reconciling = false;
    }
  };

  const unsubscribeSessions = flowChatStore.subscribe(reconcile);
  const unsubscribeScenes = useSceneStore.subscribe(reconcile);
  const unsubscribeWorkspaces = workspaceManager.addEventListener(reconcile);
  reconcile();

  return () => {
    unsubscribeSessions();
    unsubscribeScenes();
    unsubscribeWorkspaces();
    stopNavigation();
    stopProjectionSync();
  };
}
