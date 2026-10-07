const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

// Source-level assertions. The immersive bottom is a contract between the
// window (which runs full-screen), the chat page (whose fade owns the bottom
// edge), every other scrolling surface (whose viewport runs to the screen edge
// and whose content ends in a tail spacer) and each surface's fixed bottom
// controls (which keep the navigation bar clear themselves). None of that is
// visible in a unit test of a policy object, and all of it is exactly what a
// later edit is most likely to undo by accident, so it is asserted against the
// files that carry it.
function source(relativePath) {
  return fs.readFileSync(path.join(__dirname, '../..', relativePath), 'utf8');
}

const entryAbility = source('entry/src/main/ets/entryability/EntryAbility.ets');
const conversationView = source('entry/src/main/ets/pages/components/ConversationView.ets');
const windowService = source('entry/src/main/ets/services/WindowSystemBarService.ets');
const appSidebar = source('entry/src/main/ets/pages/components/AppSidebar.ets');
const miniAppSurface = source('entry/src/main/ets/pages/components/MiniAppSurface.ets');
const settingsSheet = source('entry/src/main/ets/pages/components/SettingsSheet.ets');
const workspaceToolsPanel = source('entry/src/main/ets/pages/components/WorkspaceToolsPanel.ets');
const workspacePicker = source('entry/src/main/ets/pages/components/SidebarWorkspacePicker.ets');
const connectView = source('entry/src/main/ets/pages/components/ConnectView.ets');
const filePreviewSurface = source('entry/src/main/ets/pages/components/FilePreviewSurface.ets');

// Reads one @Builder out of a component, so an assertion can name the layer it
// is about instead of counting matches in the whole file.
function builderBody(sourceText, name) {
  const start = sourceText.indexOf(`\n  private ${name}() {`);
  assert.notEqual(start, -1, `${name}() must stay a builder of the component`);
  const end = sourceText.indexOf('\n  @Builder', start);
  assert.notEqual(end, -1, `${name}() must be followed by another builder`);
  return sourceText.slice(start, end);
}

// Collapses a slice to one line so an assertion is about the call, not about
// where the formatter happened to wrap it. Comments drop out first: an
// explanation between a call's arguments is part of the source, not of the call.
function normalize(sourceText) {
  return sourceText.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/\/\/[^\n]*/g, ' ').replace(/\s+/g, ' ');
}

// Reads one chained call out of a slice so the assertions survive rewrapping.
function call(sourceText, name) {
  const start = sourceText.indexOf(`.${name}(`);
  assert.notEqual(start, -1, `the slice must keep its .${name}() call`);
  let depth = 0;
  for (let index = start + name.length + 1; index < sourceText.length; index++) {
    if (sourceText[index] === '(') {
      depth += 1;
    } else if (sourceText[index] === ')') {
      depth -= 1;
      if (depth === 0) {
        return sourceText.slice(start, index + 1).replace(/\s+/g, ' ');
      }
    }
  }
  throw new Error(`unbalanced .${name}() call`);
}

test('the shell runs the window full-screen for every page', () => {
  // Not the welcome page's private trick any more: the transcript can only reach
  // the bottom edge of the screen if the window stops reserving the strip above
  // it, and that has to hold while the chat page is the mounted page.
  assert.match(entryAbility, /mainWindow\.setWindowLayoutFullScreen\(true\)/,
    'the ability must lay the window out full-screen');
  assert.equal(entryAbility.includes('setWindowLayoutFullScreen(false)'), false,
    'no code path may put the window back into a non-immersive layout');
  assert.equal(entryAbility.includes('updateWindowLayout'), false,
    'the welcome page must not switch the window layout any more');
});

test('the navigation bar is transparent and the status bar keeps the page colour', () => {
  // The fade is what should be visible where the navigation bar sits, and the
  // header band keeps its opaque page colour so the top does not regress.
  assert.match(entryAbility, /navigationBarColor: MobileDesignColors\.transparent\.light/,
    'the navigation bar must be transparent so the page bottom shows through');
  assert.match(entryAbility, /statusBarColor: background/,
    'the status bar must keep the page colour');
});

test('the window inset service is the only place the strip is measured', () => {
  assert.match(windowService, /export class WindowInsetsBinding/,
    'the insets must be shared through one binding');
  assert.match(windowService, /bottomPadding\(designSpacing: number\): number \{[\s\S]*?Math\.max\(designSpacing, this\.bottom\)/,
    'the binding must keep the design spacing wherever it already clears the strip');
  assert.match(windowService, /appWindow\.on\('avoidAreaChange'/, 'the binding must follow avoid-area changes');
  assert.match(windowService, /appWindow\.off\('avoidAreaChange'/, 'the binding must release its listener');
});

test('the binding owns both strip numbers a surface needs', () => {
  // bottomPadding is for a fixed control: the design's spacing, or the strip if
  // the strip is larger. tailSpacing is for the end of a scrolling surface: the
  // strip plus the design's own breathing, because the last row has to rest
  // above the bar once the surface that runs under it stops scrolling.
  assert.match(windowService,
    /tailSpacing\(designSpacing: number\): number \{[\s\S]*?return this\.bottom \+ designSpacing;/,
    'the binding must expose the tail spacer a scrolling surface ends with');
});

test('the chat page bottom layer reaches the screen edge and its composer does not move', () => {
  const bottom = builderBody(conversationView, 'BottomOverlay');
  // The layer's box grows by the strip, so its own gradient fills it...
  assert.equal(call(bottom, 'padding'), '.padding({ bottom: this.bottomSafeInset })');
  // ...it may paint into the strip even where the page area still stops above
  // it, and it is named so a layout dump can be read against the pixels.
  assert.equal(call(bottom, 'expandSafeArea'),
    '.expandSafeArea([SafeAreaType.SYSTEM], [SafeAreaEdge.BOTTOM])');
  assert.equal(call(bottom, 'id'), ".id('conversation-bottom-fade')");
  // The transcript borrows the layer's measured height, which is what lets the
  // last message scroll above the fade instead of under the composer.
  assert.match(bottom, /this\.bottomInset = newArea\.height as number;/,
    'the transcript inset must keep following the layer it is measured from');
});

test('the chat page header band reserves the status bar itself', () => {
  // The shell is immersive, so the page draws from the top of the window and the
  // band is what has to keep the header out of the status bar. Its own box is
  // the transcript's content start offset, so the two stay in step.
  const top = builderBody(conversationView, 'TopOverlay');
  assert.equal(call(top, 'padding'), '.padding({ top: this.topSafeInset })');
  assert.match(top, /\.backgroundColor\(PAGE_BG_OVERLAY\)/,
    'the band that carries the inset must stay the one that paints the header');
});

test('the settings sheet viewport reaches the screen edge and its rows end in a tail spacer', () => {
  // The sheet is a bindSheet, which the framework does not inset while the
  // window is immersive, so its own box decides where content can scroll. The
  // root must not carry a bottom padding any more: that shrinks the viewport
  // and leaves a band of bare page colour under the navigation bar.
  const build = normalize(settingsSheet.slice(
    settingsSheet.indexOf('build() {'),
    settingsSheet.indexOf('@Builder', settingsSheet.indexOf('build() {'))));
  assert.equal(/\.padding\(\{[^}]*bottom/.test(build), false,
    'the sheet root must not shrink its own viewport away from the screen edge');
  assert.match(normalize(settingsSheet),
    /\.padding\(\{ left: SHEET_HORIZONTAL_PADDING, right: SHEET_HORIZONTAL_PADDING, top: 22, bottom: this\.insets\.tailSpacing\(34\) \}\)/,
    "the sheet's scrolling column must end in the strip plus its own breathing");
});

test('the mini-app gallery viewport reaches the screen edge and its grid ends in a tail spacer', () => {
  // The gallery container must not carry a bottom padding: the grid is the
  // surface's bottom edge and its tiles have to roll under the navigation bar.
  assert.match(normalize(miniAppSurface),
    /\.padding\(\{ left: 16, right: 16 \}\)/,
    'the gallery container must keep only its horizontal padding');
  assert.equal(/\.padding\(\{[^}]*bottomPadding/.test(miniAppSurface), false,
    'the gallery must not shrink its viewport away from the screen edge');
  const grid = normalize(miniAppSurface.slice(miniAppSurface.indexOf('Grid() {')));
  assert.match(grid, /GridItem\(\) \{ Column\(\) \.width\('100%'\) \.height\(this\.insets\.tailSpacing\(24\)\) \}/,
    'the grid must end with a tail spacer row');
  assert.match(grid, /\.columnStart\(0\)/,
    'the tail spacer must start at the first column');
  assert.match(grid, /\.columnEnd\(this\.galleryColumns\(\) - 1\)/,
    'the tail spacer must span to the last column');
  assert.match(grid, /\.columnsTemplate\(this\.galleryColumnsTemplate\(\)\)/,
    'the grid template and the spacer span must stay one decision');
});

test('the sidebar list scrolls under the bar and its floating footer keeps the strip clear', () => {
  // The sidebar is a scrolling session list with a floating footer, which is
  // the chat page's shape: the viewport runs to the panel's own bottom edge,
  // the footer keeps its distance from that edge itself, and the list ends in
  // a tail spacer so its last row rests above the navigation bar.
  const content = normalize(builderBody(appSidebar, 'SidebarContent'));
  assert.match(content, /\.padding\(\{ left: 20, right: 20, top: 0 \}\)/,
    'the panel root must not shrink its scrolling viewport');
  assert.match(content, /\.padding\(\{ bottom: this\.scrollTailPadding\(\) \}\)/,
    'the session list must end in a tail spacer');
  assert.match(content, /\.margin\(\{ bottom: this\.footerBottomPadding\(\) \}\)/,
    'the footer and its fade must keep the strip clear themselves');
  assert.match(normalize(appSidebar),
    /private scrollTailPadding\(\): number \{[\s\S]*?\(this\.usesCompactFooter\(\) \? 84 : 120\) \+ this\.insets\.bottom;/,
    'the tail must clear the floating footer and the strip');
  assert.match(normalize(appSidebar),
    /private footerBottomPadding\(\): number \{[\s\S]*?return this\.insets\.bottomPadding\(16\);/,
    'the footer must keep the design spacing wherever the strip is already clear');
});

test('the workspace tools sheet scrolls under the bar and its fixed controls do not', () => {
  // The tools sheet is a full-height bindSheet: its file list runs to the screen
  // edge and ends in a tail spacer, while the terminal key row and the upload
  // controls are fixed bottom controls that keep the strip clear themselves.
  const build = normalize(workspaceToolsPanel.slice(
    workspaceToolsPanel.indexOf('build() {'),
    workspaceToolsPanel.indexOf('@Builder', workspaceToolsPanel.indexOf('build() {'))));
  assert.equal(build.includes('.padding({ left: 16, right: 16, bottom'), false,
    'the panel root must not shrink its scrolling viewport');
  assert.match(normalize(workspaceToolsPanel),
    /ListItem\(\) \{ Column\(\)\.width\('100%'\)\.height\(this\.insets\.tailSpacing\(16\)\) \}/,
    'the file list must end in a tail spacer');
  assert.match(normalize(workspaceToolsPanel),
    /\.margin\(\{ bottom: this\.insets\.bottomPadding\(16\) \}\)/,
    'the fixed bottom controls must keep the strip clear');
});

test('the full-height sheets keep their fixed bottom controls out of the bar', () => {
  // These surfaces do not scroll at their bottom edge; their fixed controls
  // carry the strip (or their design spacing, where the strip is already clear)
  // so no control sits in the navigation bar's strip.
  assert.match(normalize(workspacePicker),
    /\.padding\(\{ top: 12, bottom: this\.insets\.bottomPadding\(16\) \}\)/,
    'the workspace picker confirm button must clear the strip');
  assert.match(normalize(connectView),
    /\.padding\(\{ bottom: this\.insets\.bottomPadding\(28\) \}\)/,
    'the connect sheet status strip must clear the strip');
});

test('the file preview scrollers end in strip-aware tails', () => {
  // The preview scrollers already run to the screen edge; their tails are what
  // rests the last line above the navigation bar.
  assert.match(normalize(filePreviewSurface),
    /\.padding\(\{ left: 12, right: 12, top: 14, bottom: this\.insets\.tailSpacing\(24\) \}\)/,
    'the text preview tail must clear the strip');
  assert.match(normalize(filePreviewSurface),
    /\.padding\(\{ left: 16, right: 16, top: 16, bottom: this\.insets\.tailSpacing\(28\) \}\)/,
    'the markdown preview tail must clear the strip');
});

test('every surface that owns a bottom edge reads the shared binding, not a constant', () => {
  for (const [name, text] of [['AppSidebar', appSidebar], ['MiniAppSurface', miniAppSurface],
    ['SettingsSheet', settingsSheet], ['ConversationView', conversationView],
    ['WorkspaceToolsPanel', workspaceToolsPanel], ['SidebarWorkspacePicker', workspacePicker],
    ['ConnectView', connectView], ['FilePreviewSurface', filePreviewSurface]]) {
    assert.match(text, /@Local insets: WindowInsetsBinding = new WindowInsetsBinding\(\);/,
      `${name} must hold the shared inset binding`);
    assert.match(text, /this\.insets\.bind\(this\.getUIContext\(\), context\)/,
      `${name} must bind the insets while it is mounted`);
    assert.match(text, /this\.insets\.unbind\(\)/, `${name} must release the insets when it goes`);
  }
});
