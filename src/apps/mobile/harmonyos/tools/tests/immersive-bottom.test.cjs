const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

// Source-level assertions. The immersive bottom is a contract between the
// window (which runs full-screen), the chat page (whose fade owns the bottom
// edge) and every other surface (whose interactive rows stay out of the
// navigation bar's strip). None of the three is visible in a unit test of a
// policy object, and all three are exactly what a later edit is most likely to
// undo by accident, so they are asserted against the files that carry them.
function source(relativePath) {
  return fs.readFileSync(path.join(__dirname, '../..', relativePath), 'utf8');
}

const entryAbility = source('entry/src/main/ets/entryability/EntryAbility.ets');
const conversationView = source('entry/src/main/ets/pages/components/ConversationView.ets');
const windowService = source('entry/src/main/ets/services/WindowSystemBarService.ets');
const appSidebar = source('entry/src/main/ets/pages/components/AppSidebar.ets');
const miniAppSurface = source('entry/src/main/ets/pages/components/MiniAppSurface.ets');
const settingsSheet = source('entry/src/main/ets/pages/components/SettingsSheet.ets');

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
// where the formatter happened to wrap it.
function normalize(sourceText) {
  return sourceText.replace(/\s+/g, ' ');
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

test('every other surface keeps its interactive rows out of the navigation bar', () => {
  // The rule is layered: the surface's own fill reaches the screen edge, and its
  // content stops above the strip. Each of these is the bottom-most container of
  // a surface that has controls against that edge.
  assert.match(normalize(builderBody(appSidebar, 'SidebarContent')),
    /\.padding\(\{ left: 20, right: 20, top: 0, bottom: this\.insets\.bottomPadding\(16\) \}\)/,
    'the sidebar footer must clear the strip');
  assert.match(normalize(miniAppSurface),
    /\.padding\(\{ left: 16, right: 16, bottom: this\.insets\.bottomPadding\(24\) \}\)/,
    'the mini-app gallery must clear the strip');
  assert.match(normalize(settingsSheet),
    /\.padding\(\{ bottom: this\.insets\.bottom \}\)/,
    'the settings sheet must keep its scrolling viewport above the strip');
});

test('the surfaces that avoid the strip read the shared binding, not a constant', () => {
  for (const [name, text] of [['AppSidebar', appSidebar], ['MiniAppSurface', miniAppSurface],
    ['SettingsSheet', settingsSheet], ['ConversationView', conversationView]]) {
    assert.match(text, /@Local insets: WindowInsetsBinding = new WindowInsetsBinding\(\);/,
      `${name} must hold the shared inset binding`);
    assert.match(text, /this\.insets\.bind\(this\.getUIContext\(\), context\)/,
      `${name} must bind the insets while it is mounted`);
    assert.match(text, /this\.insets\.unbind\(\)/, `${name} must release the insets when it goes`);
  }
});
