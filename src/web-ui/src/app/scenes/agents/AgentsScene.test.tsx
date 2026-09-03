// @vitest-environment jsdom

import React, { act } from 'react';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import React from 'react';
import { useAgentsStore } from './agentsStore';
import { isLocallyManageableSubagent } from './agentVisibility';

const useAgentsListMock = vi.hoisted(() => vi.fn());
const notificationInfoMock = vi.hoisted(() => vi.fn());
const notificationSuccessMock = vi.hoisted(() => vi.fn());

vi.mock('react-i18next', () => ({
  initReactI18next: {
    type: '3rdParty',
    init: vi.fn(),
  },
  useTranslation: () => ({
    t: (_key: string, options?: { defaultValue?: string }) => options?.defaultValue ?? _key,
  }),
}));

vi.mock('@/infrastructure/i18n/hooks/useI18n', () => ({
  useI18n: () => ({
    t: (key: string) => key,
  }),
}));

vi.mock('./components/CreateAgentPage', () => ({
  default: () => <div data-testid="create-agent-page">create agent</div>,
}));

vi.mock('./components/AgentCard', () => ({
  default: ({
    agent,
    toolCount,
    onOpenDetails,
  }: {
    agent: { name: string };
    toolCount?: number;
    onOpenDetails: (agent: unknown) => void;
  }) => (
    <button
      type="button"
      data-tool-count={toolCount}
      onClick={() => onOpenDetails(agent)}
    >
      {agent.name}
    </button>
  ),
}));

vi.mock('./components/CoreAgentCard', () => ({
  default: () => <div />,
}));

vi.mock('./components/useUserToolGroups', () => ({
  useUserToolGroups: () => ({
    groups: [],
    loading: false,
    saveGroups: vi.fn(),
  }),
}));

vi.mock('./components/useUserSkillGroups', () => ({
  useUserSkillGroups: () => ({
    groups: [],
    loading: false,
    saveGroups: vi.fn(),
  }),
}));

vi.mock('./components/SkillGroupPicker', () => ({
  SkillGroupPicker: () => <div data-testid="agent-detail-skill-groups">skill picker</div>,
  SkillGroupSummary: () => <div data-testid="agent-detail-skill-summary">skill summary</div>,
}));

vi.mock('./components/ToolGroupPicker', () => ({
  ToolGroupPicker: ({ tools }: { tools: Array<{ name: string }> }) => (
    <div data-testid="agent-detail-tool-groups">
      {tools.map((tool) => tool.name).join(',')}
    </div>
  ),
  ToolGroupSummary: ({ tools }: { tools: Array<{ name: string }> }) => (
    <div data-testid="agent-detail-tool-summary">
      {tools.map((tool) => tool.name).join(',')}
    </div>
  ),
}));

vi.mock('@bitfun/ui', async importOriginal => ({
  ...await importOriginal<typeof import('@bitfun/ui')>(),
  Button: ({ children, onClick }: { children: React.ReactNode; onClick?: () => void }) => (
    <button type="button" onClick={onClick}>{children}</button>
  ),
  IconButton: ({ children, onClick, 'data-testid': testId, 'aria-label': ariaLabel }: { children: React.ReactNode; onClick?: () => void; 'data-testid'?: string; 'aria-label'?: string }) => (
    <button type="button" onClick={onClick} data-testid={testId} aria-label={ariaLabel}>{children}</button>
  ),
  Select: () => <div />,
  Switch: () => <input type="checkbox" readOnly />,
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

vi.mock('@/infrastructure/confirm-dialog', async (importOriginal) => ({
  ...await importOriginal<typeof import('@/infrastructure/confirm-dialog')>(),
  confirmDanger: vi.fn(async () => false),
}));

vi.mock('@/app/components', () => ({
  GalleryDetailModal: ({ children, actions }: { children?: React.ReactNode; actions?: React.ReactNode }) => (
    <div>{children}{actions}</div>
  ),
  GalleryEmpty: () => <div />,
  GalleryGrid: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  GalleryLayout: ({ children, className }: { children: React.ReactNode; className?: string }) => (
    <main className={className}>{children}</main>
  ),
  GalleryPageHeader: ({ extraContent, actions }: { extraContent?: React.ReactNode; actions?: React.ReactNode }) => (
    <header>{extraContent}{actions}</header>
  ),
  GallerySkeleton: () => <div />,
  // Spread props so data-testid/id reach the DOM like the real GalleryZone
  // (production spreads ...sectionProps onto <section>).
  GalleryZone: ({ children, tools, ...props }: { children: React.ReactNode; tools?: React.ReactNode } & React.HTMLAttributes<HTMLElement>) => (
    <section {...props}>{tools}{children}</section>
  ),
}));

vi.mock('./hooks/useAgentsList', () => ({
  useAgentsList: () => useAgentsListMock(),
}));

function mockAgentsList(overrides: Record<string, unknown> = {}) {
  useAgentsListMock.mockReturnValue({
    allAgents: [],
    filteredAgents: [],
    loading: false,
    availableTools: [],
    toolCatalogStatus: 'available',
    getModeProfile: () => null,
    getAgentSkills: () => [],
    getModeManageableSubagents: () => [],
    counts: { builtin: 0, user: 0, project: 0, mode: 0, subagent: 0 },
    loadAgents: vi.fn(),
    getModeConfig: () => undefined,
    handleSetTools: vi.fn(),
    handleResetTools: vi.fn(),
    handleSetSkills: vi.fn(),
    handleResetSkills: vi.fn(),
    handleSetSubagentEnabled: vi.fn(),
    handleSetSubagentModel: vi.fn(),
    ...overrides,
  });
}

vi.mock('@/app/hooks/useGallerySceneAutoRefresh', () => ({
  useGallerySceneAutoRefresh: vi.fn(),
}));

vi.mock('@/infrastructure/contexts/WorkspaceContext', () => ({
  useCurrentWorkspace: () => ({ workspacePath: 'D:/workspace/project' }),
}));

vi.mock('@/infrastructure/config/services/ConfigManager', () => ({
  configManager: {
    getConfig: vi.fn(async () => false),
    onConfigChange: vi.fn(() => () => {}),
  },
}));

vi.mock('@/shared/notification-system', () => ({
  useNotification: () => ({
    success: notificationSuccessMock,
    error: vi.fn(),
    warning: vi.fn(),
    info: notificationInfoMock,
  }),
}));

vi.mock('@/infrastructure/api/service-api/SubagentAPI', () => ({
  SubagentAPI: {
    deleteSubagent: vi.fn(),
  },
}));

vi.mock('@/infrastructure/api/service-api/LegionPresetAPI', () => ({
  LegionPresetAPI: {
    createPreset: vi.fn(async () => {}),
    listPresets: vi.fn(async () => []),
  },
}));

vi.mock('./components/LegionCard', () => ({
  default: ({ pattern }: { pattern: { id: string; name: string } }) => (
    <div data-testid="legion-list-item" data-legion-id={pattern.id}>{pattern.name}</div>
  ),
}));

let JSDOMCtor: (new (
  html?: string,
  options?: { pretendToBeVisual?: boolean }
) => { window: Window & typeof globalThis }) | null = null;

try {
  const jsdom = await import('jsdom');
  JSDOMCtor = jsdom.JSDOM as typeof JSDOMCtor;
} catch {
  JSDOMCtor = null;
}

const describeWithJsdom = JSDOMCtor ? describe : describe.skip;

describeWithJsdom('AgentsScene', () => {

  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    // The jsdom environment (via the `// @vitest-environment jsdom` pragma)
    // provides a real document before react-dom initializes its event system,
    // so controlled input events dispatch like a real browser.
    container = document.createElement('div');
    document.body.appendChild(container);
    root = createRoot(container);
    vi.stubGlobal('MutationObserver', window.MutationObserver);
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe() {}
        unobserve() {}
        disconnect() {}
      },
    );
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation(() => ({
        matches: false,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
      })),
    });

    useAgentsStore.getState().openHome();
    notificationInfoMock.mockReset();
    notificationSuccessMock.mockReset();
    mockAgentsList();
  });

  afterEach(() => {
    act(() => {
      root.unmount();
    });
    container.remove();
    vi.unstubAllGlobals();
    useAgentsStore.getState().openHome();
  });

  it('keeps agent creation inside a full-height scene page wrapper', async () => {
    useAgentsStore.getState().openCreateAgent();
    const { default: AgentsScene } = await import('./AgentsScene');

    await act(async () => {
      root.render(<AgentsScene />);
    });

    expect(container.querySelector('[data-testid="create-agent-page"]')).toBeTruthy();
    expect(container.querySelector('.bitfun-agents-scene--page')).toBeTruthy();
  }, 10_000);

  it('keeps agent subpages stretched across the active scene viewport', () => {
    const stylesheet = readFileSync(
      fileURLToPath(import.meta.url).replace(/AgentsScene\.test\.tsx$/, 'AgentsScene.scss'),
      'utf8',
    );

    expect(stylesheet).toContain('width: 100%;');
    expect(stylesheet).toContain('flex: 1 1 auto;');
    expect(stylesheet).toContain('min-width: 0;');
  });

  it('uses one compact responsive catalog without overview category navigation', () => {
    const sceneSource = readFileSync(
      fileURLToPath(import.meta.url).replace(/AgentsScene\.test\.tsx$/, 'AgentsScene.tsx'),
      'utf8',
    );
    const agentCardStyles = readFileSync(
      fileURLToPath(import.meta.url).replace(/AgentsScene\.test\.tsx$/, 'components/AgentCard.scss'),
      'utf8',
    );
    const coreCardSurfaceStyles = readFileSync(
      fileURLToPath(import.meta.url).replace(/AgentsScene\.test\.tsx$/, 'components/_AgentSurfaceCard.scss'),
      'utf8',
    );
    const coreCardStyles = readFileSync(
      fileURLToPath(new URL('./components/CoreAgentCard.scss', import.meta.url)),
      'utf8',
    );
    const agentCardSource = readFileSync(
      fileURLToPath(new URL('./components/AgentCard.tsx', import.meta.url)),
      'utf8',
    );
    const coreCardSource = readFileSync(
      fileURLToPath(new URL('./components/CoreAgentCard.tsx', import.meta.url)),
      'utf8',
    );

    // One minCardWidth=360 grid remains (the custom agent teams gallery from
    // R-WF-13); the catalog grids moved to minCardWidth=300 in 1.0.0.
    expect(sceneSource.match(/<GalleryGrid\b[^>]*\bminCardWidth=\{360\}[^>]*>/g)).toHaveLength(1);
    expect(sceneSource.match(/<GalleryGrid\b[^>]*\bminCardWidth=\{300\}[^>]*>/g)).toHaveLength(1);
    expect(sceneSource).toContain('catalogAgents.map');
    expect(sceneSource).not.toContain('gallery-anchor-bar');
    expect(agentCardStyles).toMatch(/\.agent-card \{\s+width: 100%;\s+min-width: 0;/);
    expect(coreCardSurfaceStyles).toMatch(/width: 100%;\s+min-width: 0;/);
    expect(agentCardStyles).toContain('height: 148px;');
    expect(coreCardSurfaceStyles).toContain('height: 148px;');
    expect(agentCardStyles).toContain('border-radius: var(--bf-radius-lg);');
    expect(coreCardSurfaceStyles).toContain('border-radius: var(--bf-radius-lg);');
    expect(agentCardStyles).toContain('background: var(--bf-color-surface-raised);');
    expect(coreCardSurfaceStyles).toContain('background: var(--bf-color-surface-raised);');
    expect(agentCardStyles).toContain('box-shadow: var(--bf-shadow-xs);');
    expect(coreCardSurfaceStyles).toContain('box-shadow: var(--bf-shadow-xs);');
    expect(agentCardStyles).toContain('grid-template-columns: 56px minmax(0, 1fr);');
    expect(coreCardStyles).toContain('grid-template-columns: 56px minmax(0, 1fr);');
    expect(agentCardStyles).toContain('inset-block: 12px;');
    expect(coreCardStyles).toContain('inset-block: 12px;');
    expect(agentCardStyles).toContain('@container agent-card (max-width: 330px)');
    expect(coreCardStyles).toContain('@container core-agent-card (max-width: 330px)');
    expect(agentCardSource).toContain('agent-card__icon-area');
    expect(agentCardSource).toContain('agent-card__dot-field');
    expect(agentCardSource).toContain("t('agentCard.metrics.collaboration')");
    expect(coreCardSource).toContain("t('agentCard.status.connected')");
    expect(coreCardSource).toContain('core-agent-card__status');
    expect(coreCardSource).toContain('core-agent-card__dot-field');
    expect(agentCardSource).not.toContain('CAPABILITY_ACCENT');
    expect(agentCardSource).not.toContain('--agent-card-gradient');
    expect(coreCardSource).not.toContain('getAlphaColor');
    expect(coreCardSource).not.toContain('--core-card-gradient');
    expect(coreCardStyles).toMatch(/&__status \{[\s\S]*?color: var\(--bf-color-content-primary\);[\s\S]*?\.core-agent-card__status-icon \{[\s\S]*?color: var\(--bf-color-status-success-content\);/);
    expect(coreCardSurfaceStyles).not.toContain('$gradient');
    expect(coreCardSurfaceStyles).toContain('@mixin agent-icon-dot-field()');
    expect(coreCardSurfaceStyles).toContain('background-size: 7px 7px;');
    expect(coreCardSurfaceStyles).toContain('mask-image: linear-gradient(to bottom, currentColor 0%, transparent 100%);');
    expect(agentCardStyles).not.toContain('width: 360px;');
    expect(coreCardSurfaceStyles).not.toContain('width: 360px;');
  });

  it('shows Harness as a three-stop rail with a separate creative direction', async () => {
    const { default: AgentsScene } = await import('./AgentsScene');

    await act(async () => {
      root.render(<AgentsScene />);
    });

    const minimal = container.querySelector<HTMLElement>('[data-testid="agents-harness-minimal"]');
    const balanced = container.querySelector<HTMLElement>('[data-testid="agents-harness-balanced"]');
    const ultimate = container.querySelector<HTMLElement>('[data-testid="agents-harness-ultimate"]');
    const creative = container.querySelector<HTMLElement>('[data-testid="agents-harness-creative"]');
    expect(container.querySelectorAll('.bitfun-agents-scene__harness-profile')).toHaveLength(3);
    expect(container.querySelectorAll('.bitfun-agents-scene__harness-rail-node')).toHaveLength(3);
    expect(container.querySelectorAll('.bitfun-agents-scene__harness-rail-node.is-default')).toHaveLength(1);
    expect(container.querySelector('.bitfun-agents-scene__harness-presentation')).toBeTruthy();
    expect(container.querySelector('.bitfun-agents-scene__harness-track')).toBeTruthy();
    expect(container.querySelector('.bitfun-agents-scene__harness-creative')).toBe(creative);
    expect(container.querySelector('.bitfun-agents-scene__harness-step')).toBeNull();
    expect(container.querySelector('.bitfun-agents-scene__harness-card')).toBeNull();
    expect(minimal).toBeTruthy();
    expect(minimal?.tagName).toBe('DIV');
    expect(container.querySelector('.bitfun-agents-scene__harness-step-status')).toBeNull();
    expect(minimal?.dataset.bfState).toBeUndefined();
    expect(ultimate?.dataset.bfState).toBeUndefined();
    expect(minimal?.dataset.harnessGear).toBe('1');
    expect(balanced?.dataset.harnessGear).toBe('2');
    expect(ultimate?.dataset.harnessGear).toBe('3');
    expect(creative?.dataset.harnessGear).toBe('creative');
    expect(minimal?.textContent).toContain('harnessZone.profiles.minimal.purpose');
    expect(minimal?.textContent).not.toContain('harnessZone.connected');
    expect(ultimate?.textContent).not.toContain('harnessZone.comingSoon');
    expect(creative?.querySelector('[data-bf-name="creative"]')).toBeTruthy();

    expect(notificationInfoMock).not.toHaveBeenCalled();
    expect(notificationSuccessMock).not.toHaveBeenCalled();
  });

  it('shows skill grouping and editing for a custom subagent with the Skill tool', async () => {

    const subagent = {
      key: 'user::skill-worker',
      id: 'skill-worker',
      name: 'Skill worker',
      description: 'Uses specialized workflows.',
      isReadonly: false,
      isReview: false,
      toolCount: 1,
      defaultTools: ['Skill'],
      defaultEnabled: true,
      effectiveEnabled: true,
      source: 'user',
      agentKind: 'subagent' as const,
      capabilities: [],
    };
    mockAgentsList({
      allAgents: [subagent],
      filteredAgents: [subagent],
      getAgentSkills: (agentId: string) => agentId === subagent.id
        ? [{ key: 'user::custom::workflow', effectiveEnabled: true }]
        : [],
    });
    const { default: AgentsScene } = await import('./AgentsScene');

    await act(async () => {
      root.render(<AgentsScene />);
    });
    await act(async () => {
      Array.from(container.querySelectorAll<HTMLButtonElement>('button'))
        .find((button) => button.textContent === subagent.name)
        ?.click();
    });

    expect(container.querySelector('[data-testid="agent-detail-configuration"]')).toBeTruthy();
    expect(container.querySelector('[data-testid="agent-detail-overview"]')).toBeNull();
    expect(container.querySelector('.agent-card__detail-view-tabs')).toBeNull();

    const skillsTab = container.querySelector<HTMLButtonElement>('[data-detail-section="skills"]');
    expect(skillsTab).toBeTruthy();

    await act(async () => {
      skillsTab?.click();
    });
    expect(container.querySelector('[data-testid="agent-detail-skill-summary"]')).toBeTruthy();

    const manageButton = Array.from(container.querySelectorAll<HTMLButtonElement>('button'))
      .find((button) => button.textContent === 'manage');
    expect(manageButton).toBeTruthy();
    await act(async () => {
      manageButton?.click();
    });
    expect(container.querySelector('[data-testid="agent-detail-skill-groups"]')).toBeTruthy();
  });

  // Guard the two historical break-points: the create entry (L1-P0-1: the
  // create_legion_preset command was never registered on the Rust side) and
  // the disabled save button (L1-P0-2: LEGION_CREATE_BACKEND_READY=false).
  // Plus the LegionCard gallery (L1-P1-1 wiring).

  it('renders the create-legion entry button and opens the CreateLegionPage', async () => {
    const { default: AgentsScene } = await import('./AgentsScene');

    await act(async () => {
      root.render(<AgentsScene />);
    });

    const createBtn = container.querySelector<HTMLButtonElement>('[data-testid="agents-create-legion-btn"]');
    expect(createBtn).toBeTruthy();

    await act(async () => {
      createBtn?.click();
    });
    expect(container.querySelector('[data-testid="create-legion-page"]')).toBeTruthy();
  }, 10_000);

  it('keeps the CreateLegionPage save button enabled (P0-2 regression)', async () => {
    const { default: AgentsScene } = await import('./AgentsScene');

    await act(async () => {
      root.render(<AgentsScene />);
    });
    // Open the create-legion page through the same button the user clicks
    // (L1-P0-2 regression: the save button used to be hard-disabled).
    const createBtn = container.querySelector<HTMLButtonElement>('[data-testid="agents-create-legion-btn"]');
    await act(async () => {
      createBtn?.click();
    });

    const saveBtn = container.querySelector<HTMLButtonElement>('[data-testid="create-legion-save"]');
    expect(saveBtn).toBeTruthy();
    expect(saveBtn?.disabled).toBe(false);
    // Pattern options are rendered from the built-in patterns list.
    expect(container.querySelectorAll('[data-testid="legion-pattern-option"]').length).toBeGreaterThan(0);
  }, 10_000);

  it('exposes the pattern selector as a radiogroup and fires on Space key (鍓嶇-P2-3)', async () => {
    const { default: AgentsScene } = await import('./AgentsScene');

    await act(async () => {
      root.render(<AgentsScene />);
    });
    const createBtn = container.querySelector<HTMLButtonElement>('[data-testid="agents-create-legion-btn"]');
    await act(async () => {
      createBtn?.click();
    });

    // Single-select semantics: group is a radiogroup, options are radios with aria-checked.
    const group = container.querySelector('[role="radiogroup"]');
    expect(group).toBeTruthy();
    const options = [...container.querySelectorAll('[role="radio"]')] as HTMLElement[];
    expect(options.length).toBeGreaterThan(0);
    expect(options.filter((o) => o.getAttribute('aria-checked') === 'true').length).toBe(1);

    // Space key must select a non-active option (button semantics: Enter + Space).
    const inactive = options.find((o) => o.getAttribute('aria-checked') !== 'true');
    expect(inactive).toBeTruthy();
    await act(async () => {
      inactive!.dispatchEvent(new window.KeyboardEvent('keydown', { key: ' ', bubbles: true }));
    });
    const selected = [...container.querySelectorAll('[role="radio"]')].find(
      (o) => o.getAttribute('aria-checked') === 'true',
    );
    expect(selected?.getAttribute('data-pattern-id')).toBe(inactive?.getAttribute('data-pattern-id'));
  }, 10_000);

  it('announces the pattern summary through aria-live (鍓嶇-P2-4)', async () => {
    const { default: AgentsScene } = await import('./AgentsScene');

    await act(async () => {
      root.render(<AgentsScene />);
    });
    const createBtn = container.querySelector<HTMLButtonElement>('[data-testid="agents-create-legion-btn"]');
    await act(async () => {
      createBtn?.click();
    });

    // The summary section that changes on pattern switch is polite/atomic.
    const liveRegions = [...container.querySelectorAll('[aria-live="polite"]')] as HTMLElement[];
    expect(liveRegions.length).toBeGreaterThan(0);
    expect(liveRegions.some((r) => r.getAttribute('aria-atomic') === 'true')).toBe(true);
  }, 10_000);

  it('renders the DAG canvas preview on the CreateLegionPage (R-WF-17 assertion 1)', async () => {
    const { default: AgentsScene } = await import('./AgentsScene');

    await act(async () => {
      root.render(<AgentsScene />);
    });
    const createBtn = container.querySelector<HTMLButtonElement>('[data-testid="agents-create-legion-btn"]');
    await act(async () => {
      createBtn?.click();
    });

    const canvas = container.querySelector('[data-testid="legion-pattern-canvas"]');
    expect(canvas).toBeTruthy();
    expect(canvas?.querySelector('svg')).toBeTruthy();
  }, 10_000);

  it('marks the createLegion page with the agents scene-root contract (鍓嶇-P2-6)', async () => {
    const { default: AgentsScene } = await import('./AgentsScene');

    await act(async () => {
      root.render(<AgentsScene />);
    });
    const createBtn = container.querySelector<HTMLButtonElement>('[data-testid="agents-create-legion-btn"]');
    await act(async () => {
      createBtn?.click();
    });

    const pageRoot = container.querySelector('[data-testid="create-legion-page"]')?.parentElement;
    expect(pageRoot?.getAttribute('data-bf-scene')).toBe('agents');
    expect(pageRoot?.getAttribute('data-bf-part')).toBe('root');
  }, 10_000);

  it('uses the unified back-to-overview label on both editor pages (P2-4)', async () => {
    const legionSource = readFileSync(
      fileURLToPath(import.meta.url).replace(/AgentsScene\.test\.tsx$/, 'components/CreateLegionPage.tsx'),
      'utf8',
    );
    const agentSource = readFileSync(
      fileURLToPath(import.meta.url).replace(/AgentsScene\.test\.tsx$/, 'components/CreateAgentPage.tsx'),
      'utf8',
    );

    // Both editors return to the same overview, so both must resolve the back
    // label through the same i18n key instead of divergent copy (P2-4).
    expect(legionSource).not.toContain('legionPattern.back');
    expect(legionSource).toContain('agentsOverview.backToOverview');
    expect(agentSource).toContain('agentsOverview.backToOverview');

    const { default: AgentsScene } = await import('./AgentsScene');

    await act(async () => {
      root.render(<AgentsScene />);
    });
    const createLegionBtn = container.querySelector<HTMLButtonElement>('[data-testid="agents-create-legion-btn"]');
    await act(async () => {
      createLegionBtn?.click();
    });

    const headerBack = container.querySelector('[data-testid="create-legion-back"]');
    expect(headerBack?.getAttribute('aria-label')).toBe('agentsOverview.backToOverview');
    const actionButtons = [...container.querySelectorAll('.create-agent-page__actions button')] as HTMLButtonElement[];
    expect(actionButtons.some((b) => b.textContent === 'agentsOverview.backToOverview')).toBe(true);
  }, 10_000);

  it('renders saved legion presets through the LegionCard gallery (P1-1 wiring)', async () => {
    const { LegionPresetAPI } = await import('@/infrastructure/api/service-api/LegionPresetAPI');
    const listPresets = LegionPresetAPI.listPresets as ReturnType<typeof vi.fn>;
    listPresets.mockResolvedValue([
      {
        id: 'sparc-dev',
        name: 'SPARC Development',
        description: '5-stage SPARC development pipeline',
        nodes: [{ id: 'researcher', agent: 'Plan', role: 'Research Bee', prompt: 'Gather requirements' }],
        edges: [],
      },
    ]);
    const { default: AgentsScene } = await import('./AgentsScene');

    await act(async () => {
      root.render(<AgentsScene />);
    });
    // Flush the listPresets() promise chain (effect -> resolve -> setState -> re-render).
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    const zone = container.querySelector('[data-testid="agents-legions-zone"]');
    expect(zone).toBeTruthy();
    const card = container.querySelector('[data-testid="legion-list-item"]');
    expect(card).toBeTruthy();
    expect(card?.getAttribute('data-legion-id')).toBe('sparc-dev');
  }, 10_000);

  it('keeps MCP tools out of mode cards and tool details', async () => {
    const mode = {
      key: 'mode::custom-mode',
      id: 'custom-mode',
      name: 'Custom mode',
      description: 'General coding mode.',
      isReadonly: false,
      isReview: false,
      toolCount: 2,
      defaultTools: ['Read'],
      defaultEnabled: true,
      effectiveEnabled: true,
      source: 'user',
      agentKind: 'mode' as const,
      capabilities: [],
    };
    mockAgentsList({
      allAgents: [mode],
      filteredAgents: [mode],
      availableTools: [
        { name: 'Read', description: 'Read files.', is_readonly: true },
        {
          name: 'mcp__github__list_issues',
          description: 'List issues.',
          is_readonly: true,
        },
      ],
      getModeConfig: () => ({
        profile_id: 'coding_shared',
        enabled_tools: ['Read', 'mcp__github__list_issues'],
        default_tools: ['Read'],
      }),
    });
    const { default: AgentsScene } = await import('./AgentsScene');

    await act(async () => {
      root.render(<AgentsScene />);
    });

    const card = Array.from(container.querySelectorAll<HTMLButtonElement>('button'))
      .find((button) => button.textContent === mode.name);
    expect(card?.dataset.toolCount).toBe('1');

    await act(async () => {
      card?.click();
    });

    await act(async () => {
      container.querySelector<HTMLButtonElement>('[data-detail-section="tools"]')?.click();
    });

    const summary = container.querySelector('[data-testid="agent-detail-tool-summary"]');
    expect(summary?.textContent).toBe('Read');
    expect(summary?.textContent).not.toContain('mcp__github__list_issues');
  });

  it('surfaces an unsupported tool catalog in the tools tab instead of an empty list', async () => {
    // When the host can't answer get_all_tools_info the tools tab must say so
    // and disable editing, rather than rendering as "no tools". See PR #2428
    // round 5 #2.
    const mode = {
      key: 'mode::custom-mode',
      id: 'custom-mode',
      name: 'Custom mode',
      description: 'General coding mode.',
      isReadonly: false,
      isReview: false,
      toolCount: 1,
      defaultTools: ['Read'],
      defaultEnabled: true,
      effectiveEnabled: true,
      source: 'user',
      agentKind: 'mode' as const,
      capabilities: [],
    };
    mockAgentsList({
      allAgents: [mode],
      filteredAgents: [mode],
      availableTools: [],
      toolCatalogStatus: 'unsupported',
      getModeConfig: () => ({
        profile_id: 'custom-mode',
        enabled_tools: ['Read'],
        default_tools: ['Read'],
      }),
    });
    const { default: AgentsScene } = await import('./AgentsScene');

    await act(async () => {
      root.render(<AgentsScene />);
    });
    await act(async () => {
      Array.from(container.querySelectorAll<HTMLButtonElement>('button'))
        .find((button) => button.textContent === mode.name)
        ?.click();
    });
    await act(async () => {
      container.querySelector<HTMLButtonElement>('[data-detail-section="tools"]')
        ?.click();
    });

    const status = container.querySelector('[data-testid="agent-detail-tools-catalog-status"]');
    expect(status?.textContent).toContain('agentsOverview.toolsUnsupported');
    // The tool summary picker must not render — the catalog is not available.
    expect(container.querySelector('[data-testid="agent-detail-tool-summary"]')).toBeNull();
  });
});
