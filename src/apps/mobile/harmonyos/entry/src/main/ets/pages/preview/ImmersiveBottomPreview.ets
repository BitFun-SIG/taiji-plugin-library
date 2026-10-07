import { ObservableChatTimelineItem } from '../../model/ChatTimelineModels';
import { ChatMessage } from '../../model/RemoteModels';
import { MiniAppSummary } from '../../model/MiniAppModels';
import { ChatSurface } from '../state/ChatSurface';
import { REMOTE_CHAT_COMPOSER_CAPABILITIES } from '../state/ChatComposerCapabilities';
import { AppShellState } from '../state/AppShellState';
import { MiniAppState } from '../state/MiniAppState';
import { ConversationView } from '../components/ConversationView';
import { AppShell } from '../components/AppShell';
import { MiniAppSurface } from '../components/MiniAppSurface';
import { AppSettingsSurface, AppSidebarSurface } from '../components/AppRootOverlaySurfaces';
import { emptyAppRootPresentationActions } from '../actions/AppRootPresentationActions';
import { ComposerPresentation } from '../components/ComposerBar';
import { ConversationViewOptions } from '../components/ConversationViewContract';
import { ConversationViewState } from '../state/ConversationViewState';
import { ConversationIntent } from '../actions/ConversationIntent';

/** Turns in the transcript. Long enough that the list has to scroll to reach the end. */
const TRANSCRIPT_TURNS: number = 16;

/** Tiles in the mini-app gallery: enough rows that the gallery itself scrolls. */
const GALLERY_APPS: number = 9;

/** Prose tall enough to be read while it is already half faded out. */
const ANSWER_PARAGRAPH: string =
  'The transcript runs the full height of the pane, so the last answer keeps '
  + 'scrolling until it reaches the bottom edge of the screen. The composer '
  + 'floats over it and the fade carries the text into the page colour instead '
  + 'of cutting it off at a hard line.';

const FINAL_ANSWER_PARAGRAPH: string =
  'This is the final answer of the transcript, and it decides whether the bottom '
  + 'edge reads as a continuation of the conversation or as a strip of empty page '
  + 'colour: it must be able to scroll above the floating composer, and everything '
  + 'below the composer must still look like the surface the answer is drawn on. A '
  + 'reader who flings the list to the end should see the answer fade into the page '
  + 'rather than stop short of it.';

function message(id: string, role: string, text: string): ChatMessage {
  return { id, role, text, status: 'completed', timestamp: '0' };
}

function row(id: string, role: string, text: string): ObservableChatTimelineItem {
  return new ObservableChatTimelineItem({
    id,
    type: role === 'user' ? 'user_message' : 'assistant_message',
    message: message(id, role, text),
    isStreaming: false,
    isFinalizing: false
  });
}

function transcript(): ObservableChatTimelineItem[] {
  const rows: ObservableChatTimelineItem[] = [];
  for (let index = 0; index < TRANSCRIPT_TURNS; index++) {
    const turn = index + 1;
    rows.push(row(`turn-${turn}-user`, 'user', `Round ${turn}: how does the bottom of the chat page read?`));
    rows.push(row(`turn-${turn}-assistant`, 'assistant',
      turn === TRANSCRIPT_TURNS ? FINAL_ANSWER_PARAGRAPH : ANSWER_PARAGRAPH));
  }
  return rows;
}

function gallery(): MiniAppSummary[] {
  const apps: MiniAppSummary[] = [];
  for (let index = 0; index < GALLERY_APPS; index++) {
    const tile = index + 1;
    apps.push({
      id: `gallery-${tile}`,
      name: `Mini app ${tile}`,
      description: 'Offline sample tile, so the gallery has rows below the fold.'
    });
  }
  return apps;
}

/**
 * The chat page's bottom edge, drawn by the production `ConversationView` over a
 * transcript long enough to scroll: the composer and the status line float over
 * the list, and the list carries the page colour down to the bottom of the
 * screen.
 *
 * This fixture exists because the bottom edge cannot be judged from the composer
 * alone: where the input ends up, how far the last message can scroll, and which
 * colour the fade finishes on are three separate readings of one capture, and the
 * layout dump has to show the same pane the pixels do.
 *
 * The scenario id picks the state, because the shell has to be in it before the
 * first frame: `-wide` swaps the composer to the wide detail pane's floating
 * presentation, `-drawer` mounts the production `AppShell` with its drawer open
 * over the same conversation, `-settings` and `-cover` mount the shell with its
 * settings sheet and its compact account cover open, and `-miniapps` mounts the
 * mini-app gallery. None of them changes the window posture: every one of these
 * surfaces shares the shell's immersive layout, so every one of them is where a
 * control can end up under the navigation bar.
 */
@ComponentV2
export struct ImmersiveBottomPreview {
  @Local state: ConversationViewState = this.initialState();
  @Local shellState: AppShellState = this.initialShellState();
  @Local miniAppState: MiniAppState = this.initialMiniAppState();
  @Local floating: boolean = false;
  @Local drawer: boolean = false;
  @Local settings: boolean = false;
  @Local cover: boolean = false;
  @Local galleryVisible: boolean = false;
  @Local wideShell: boolean = false;

  aboutToAppear(): void {
    const scenarioId = AppStorage.get<string>('scenarioId') ?? '';
    this.floating = scenarioId.endsWith('-wide');
    this.drawer = scenarioId.endsWith('-drawer');
    this.settings = scenarioId.endsWith('-settings');
    this.cover = scenarioId.endsWith('-cover');
    this.galleryVisible = scenarioId.endsWith('-miniapps');
    // The wide shell keeps the same session list as a master pane instead of a
    // drawer, so its bottom edge is the same bottom edge.
    this.wideShell = scenarioId.endsWith('-wide-shell');
    // The shell's own surfaces have to be open before the first frame, and the
    // scenario id is only known once the page is appearing, so the flags are
    // applied here rather than in the field initialisers.
    this.shellState.showSidebar = this.drawer || this.wideShell;
    this.shellState.showSettings = this.settings || this.cover;
    if (this.cover) {
      this.shellState.settingsMode = 'account';
    }
  }

  private initialState(): ConversationViewState {
    const state = new ConversationViewState();
    state.surface = ChatSurface.Remote;
    state.connectionState = 'connected';
    state.activeSession = {
      sessionId: 'immersive-bottom-preview',
      title: 'Bottom edge',
      workspacePath: '/workspace',
      agentType: 'code'
    };
    state.desktopName = 'Studio';
    state.composerCapabilities = REMOTE_CHAT_COMPOSER_CAPABILITIES;
    state.timelineItems = transcript();
    state.timelineRevision = 1;
    return state;
  }

  private initialShellState(): AppShellState {
    return new AppShellState();
  }

  private initialMiniAppState(): MiniAppState {
    const state = new MiniAppState();
    state.visible = true;
    state.apps = gallery();
    return state;
  }

  private options(): ConversationViewOptions {
    const options = new ConversationViewOptions();
    options.showSidebarButton = true;
    options.showBackButton = false;
    options.composerPresentation =
      this.floating ? ComposerPresentation.Floating : ComposerPresentation.Compact;
    return options;
  }

  build() {
    if (this.galleryVisible) {
      MiniAppSurface({ state: this.miniAppState })
    } else if (this.drawer || this.settings || this.cover || this.wideShell) {
      this.ShellLayout()
    } else {
      this.Conversation()
    }
  }

  /**
   * The drawer, the settings sheet and the compact account cover are all shell
   * surfaces rather than pages, so they are mounted through the production shell
   * with the production content for their slot. Each of them has its own bottom
   * edge, and the shell is what moves them with the window.
   */
  @Builder
  private ShellLayout() {
    AppShell({
      shellState: this.shellState,
      useWideLayout: this.wideShell,
      allowSidebar: true,
      compactAccountLogin: this.cover,
      content: () => { this.Conversation(); },
      sidebar: () => { this.Sidebar(); },
      settings: () => { this.Settings(); },
      onCloseSidebar: () => { this.shellState.showSidebar = false; }
    })
  }

  @Builder
  private Settings() {
    AppSettingsSurface({ shellState: this.shellState })
  }

  @Builder
  private Sidebar() {
    AppSidebarSurface({
      shellState: this.shellState,
      actions: emptyAppRootPresentationActions()
    })
  }

  @Builder
  private Conversation() {
    ConversationView({
      state: this.state,
      options: this.options(),
      onIntent: (_intent: ConversationIntent): void => {}
    })
  }
}
