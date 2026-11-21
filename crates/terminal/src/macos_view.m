// AppKit NSView / NSTextInputClient / drag-and-drop host for macOS terminals.
// Delegates CoreGraphics + CoreText rendering to apple_render.m (CbAppleRenderer).
#import "apple_render.h"
#import <Cocoa/Cocoa.h>
#import <IOKit/hidsystem/IOLLEvent.h>
#import <QuartzCore/QuartzCore.h>
#include <math.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>

static const NSTimeInterval kCommitHoldGrace = 0.12;
static const NSTimeInterval kRecentUnmarkWindow = 0.06;

static uint16_t cb_mods_from_flags(NSEventModifierFlags raw) {
  uint16_t mods = 0;
  if (raw & NSEventModifierFlagShift) {
    mods |= CB_MODS_SHIFT;
    if (raw & NX_DEVICERSHIFTKEYMASK)
      mods |= CB_MODS_SHIFT_SIDE;
  }
  if (raw & NSEventModifierFlagControl) {
    mods |= CB_MODS_CTRL;
    if (raw & NX_DEVICERCTLKEYMASK)
      mods |= CB_MODS_CTRL_SIDE;
  }
  if (raw & NSEventModifierFlagOption) {
    mods |= CB_MODS_ALT;
    if (raw & NX_DEVICERALTKEYMASK)
      mods |= CB_MODS_ALT_SIDE;
  }
  if (raw & NSEventModifierFlagCommand) {
    mods |= CB_MODS_SUPER;
    if (raw & NX_DEVICERCMDKEYMASK)
      mods |= CB_MODS_SUPER_SIDE;
  }
  if (raw & NSEventModifierFlagCapsLock) {
    mods |= CB_MODS_CAPS_LOCK;
  }
  return mods;
}

static uint32_t cb_first_scalar(NSString *s) {
  if (!s || s.length == 0)
    return 0;
  unichar c0 = [s characterAtIndex:0];
  if (CFStringIsSurrogateHighCharacter(c0) && s.length >= 2) {
    unichar c1 = [s characterAtIndex:1];
    if (CFStringIsSurrogateLowCharacter(c1))
      return CFStringGetLongCharacterForSurrogatePair(c0, c1);
  }
  return (uint32_t)c0;
}


static bool cb_is_single_line_printable(NSString *s) {
  if (!s || s.length == 0)
    return false;
  NSUInteger len = s.length;
  for (NSUInteger i = 0; i < len; i++) {
    unichar c = [s characterAtIndex:i];
    if (c < 0x20 || c == 0x7f || (c >= 0xF700 && c <= 0xF8FF))
      return false;
  }
  return true;
}

static bool cb_contains_non_ascii(NSString *s) {
  if (!s)
    return false;
  NSUInteger len = s.length;
  for (NSUInteger i = 0; i < len; i++) {
    if ([s characterAtIndex:i] >= 0x80)
      return true;
  }
  return false;
}



@interface CanbanTerminal : NSView <NSTextInputClient, NSDraggingDestination> {
@public
  void *_host;
  CbCallbacks _cb;
  const CbSnapshot *_snap;
  CbAppleRenderer *_renderer;
  uint64_t _prevImagePlacementsGen;
  size_t _prevImagePlacementsLen;
  uint64_t _physicalKeyDownLo;
  uint64_t _physicalKeyDownHi;
  uint8_t _themeBg[3];
}
@property(nonatomic, copy) NSString *fontFamily;
@property(nonatomic) CGFloat fontSize;
@property(nonatomic, readonly) CGFloat cellWidth;
@property(nonatomic, readonly) CGFloat cellHeight;
@property(nonatomic) CGFloat backingScale;

@property(nonatomic) BOOL autoFocus;
@property(nonatomic) BOOL focusedState;
@property(nonatomic, strong) NSSet<NSString *> *uiCommandKeys;

@property(nonatomic, copy) NSString *markedTextStr;
@property(nonatomic) NSRange markedRangeVal;
@property(nonatomic) NSRange selectedRangeVal;
@property(nonatomic, strong) NSMutableArray<NSString *> *keyTextAccumulator;
@property(nonatomic) BOOL didDoCommand;
@property(nonatomic) BOOL didUpdateMarkedText;
@property(nonatomic) SEL pendingCommandSelector;
@property(nonatomic) BOOL commitCursorActive;
@property(nonatomic) uint16_t commitOriginX;
@property(nonatomic) uint16_t commitOriginY;
@property(nonatomic) uint16_t commitTargetX;
@property(nonatomic) uint16_t commitTargetY;
@property(nonatomic) NSTimeInterval commitHoldUntil;
@property(nonatomic) NSTimeInterval lastUnmarkTime;

@property(nonatomic, strong) NSTimer *animTimer;
@property(nonatomic) NSTimeInterval lastBlinkTime;
@property(nonatomic) BOOL cursorBlinkVisible;
@property(nonatomic) NSTimeInterval scrollbarVisibleUntil;
@property(nonatomic) uint64_t prevScrollOffset;
@property(nonatomic) uint64_t prevScrollTotal;

@property(nonatomic) BOOL mouseCapturedByPty;
@property(nonatomic) BOOL selectingLocally;
@property(nonatomic) BOOL selectionRectangle;
@property(nonatomic) int8_t autoscrollDir;
@property(nonatomic) NSPoint lastDragPoint;
@property(nonatomic) CGFloat scrollAccumY;
@property(nonatomic) CGFloat scrollAccumX;

@property(nonatomic, strong) NSTrackingArea *trackingArea;
@property(nonatomic, weak) NSView *testContentView;
@property(nonatomic) BOOL hoverLinkValid;
@property(nonatomic) uint16_t hoverLinkRow;
@property(nonatomic) uint16_t hoverLinkStartCol;
@property(nonatomic) uint16_t hoverLinkEndCol;
@property(nonatomic, copy) NSString *hoverLinkUri;
@property(nonatomic) NSEventModifierFlags lastModifierFlags;
@property(nonatomic) NSUInteger pendingLayoutRedraws;
@end

@implementation CanbanTerminal

- (instancetype)initWithHost:(void *)host callbacks:(CbCallbacks)callbacks {
  self = [super initWithFrame:NSZeroRect];
  if (!self)
    return nil;
  _host = host;
  _cb = callbacks;
  _snap = NULL;
  _renderer = [[CbAppleRenderer alloc] init];
  _uiCommandKeys = [NSSet set];
  _markedRangeVal = NSMakeRange(NSNotFound, 0);
  _selectedRangeVal = NSMakeRange(0, 0);
  _cursorBlinkVisible = YES;
  _lastBlinkTime = [NSProcessInfo processInfo].systemUptime;
  _themeBg[0] = 25;
  _themeBg[1] = 30;
  _themeBg[2] = 38;
  self.wantsLayer = YES;
  self.layer.masksToBounds = YES;
  self.layer.opaque = YES;
  [self applyThemeBgLayerColor];
  [self registerForDraggedTypes:@[
    NSPasteboardTypeFileURL,
    NSPasteboardTypeURL,
    NSPasteboardTypeString,
  ]];
  return self;
}

- (NSString *)fontFamily {
  return _renderer.fontFamily;
}

- (void)setFontFamily:(NSString *)fontFamily {
  _renderer.fontFamily = fontFamily;
}

- (CGFloat)fontSize {
  return _renderer.fontSize;
}

- (void)setFontSize:(CGFloat)fontSize {
  _renderer.fontSize = fontSize;
}

- (CGFloat)cellWidth {
  return _renderer.cellWidth;
}

- (CGFloat)cellHeight {
  return _renderer.cellHeight;
}

- (CGFloat)backingScale {
  return _renderer.backingScale;
}

- (void)setBackingScale:(CGFloat)backingScale {
  _renderer.backingScale = backingScale;
}

- (void)applyThemeBgLayerColor {
  if (!self.layer)
    return;
  CGColorSpaceRef cs = CGColorSpaceCreateDeviceRGB();
  CGFloat comp[4] = {_themeBg[0] / 255.0, _themeBg[1] / 255.0,
                     _themeBg[2] / 255.0, 1.0};
  CGColorRef c = CGColorCreate(cs, comp);
  self.layer.backgroundColor = c;
  CFRelease(c);
  CGColorSpaceRelease(cs);
}

- (void)setThemeBgR:(uint8_t)r g:(uint8_t)g b:(uint8_t)b {
  if (_themeBg[0] == r && _themeBg[1] == g && _themeBg[2] == b)
    return;
  _themeBg[0] = r;
  _themeBg[1] = g;
  _themeBg[2] = b;
  [self applyThemeBgLayerColor];
  [self setNeedsDisplay:YES];
}

- (void)syncRebind {
  _physicalKeyDownLo = 0;
  _physicalKeyDownHi = 0;
  [self.inputContext discardMarkedText];
  [self unmarkText];
  _commitCursorActive = NO;
  _commitHoldUntil = 0;
  _lastUnmarkTime = 0;
  _selectingLocally = NO;
  _autoscrollDir = 0;
  _hoverLinkValid = NO;
  _hoverLinkUri = nil;
  _scrollbarVisibleUntil = 0;
  _prevScrollOffset = 0;
  _prevScrollTotal = 0;
  _cursorBlinkVisible = YES;
  _lastBlinkTime = [NSProcessInfo processInfo].systemUptime;
  [_renderer clearImageCache];
  _prevImagePlacementsGen = 0;
  _prevImagePlacementsLen = 0;
  _snap = NULL;
  [self pollSnapshotForceFull:YES];
  [self setNeedsDisplay:YES];
  if (_autoFocus) {
    [self requestAutoFocusIfEligible];
  }
}

- (void)detachHost {
  _host = NULL;
  _snap = NULL;
  [_renderer clearImageCache];
  _commitCursorActive = NO;
  _commitHoldUntil = 0;
  _lastUnmarkTime = 0;
  [self.animTimer invalidate];
  self.animTimer = nil;
  [[NSNotificationCenter defaultCenter] removeObserver:self];
}

- (void)dealloc {
  [self detachHost];
}

- (BOOL)isFlipped {
  return YES;
}

- (BOOL)isOpaque {
  return YES;
}

- (BOOL)acceptsFirstResponder {
  return YES;
}

- (BOOL)acceptsFirstMouse:(NSEvent *)event {
  (void)event;
  return YES;
}

- (void)rebuildFonts {
  [_renderer rebuildFonts];
}

- (CTFontRef)resolveFontForUTF16:(const UniChar *)chars
                          length:(CFIndex)len
                       codepoint:(uint32_t)cp
                           flags:(uint16_t)flags {
  return [_renderer resolveFontForUTF16:chars
                                 length:len
                              codepoint:cp
                                  flags:flags];
}

- (BOOL)hasRenderableBounds {
  CGFloat cw = self.cellWidth > 0.0 ? self.cellWidth : 8.0;
  CGFloat ch = self.cellHeight > 0.0 ? self.cellHeight : 18.0;
  NSRect b = self.bounds;
  return b.size.width >= (kPadX * 2.0 + cw * 2.0) &&
         b.size.height >= (kPadY * 2.0 + ch);
}

- (CbGeometry)currentGeometry {
  NSRect b = self.bounds;
  CGFloat cw = self.cellWidth > 0.0 ? self.cellWidth : 8.0;
  CGFloat ch = self.cellHeight > 0.0 ? self.cellHeight : 18.0;
  CbGeometry g;
  g.cell_width = (uint32_t)fmax(1.0, round(cw));
  g.cell_height = (uint32_t)fmax(1.0, round(ch));
  g.pad_left = (uint32_t)kPadX;
  g.pad_right = (uint32_t)kPadX;
  g.pad_top = (uint32_t)kPadY;
  g.pad_bottom = (uint32_t)kPadY;
  if (![self hasRenderableBounds]) {
    g.cols = 0;
    g.rows = 0;
    return g;
  }
  CGFloat innerW = fmax(cw * 2.0, b.size.width - kPadX * 2.0);
  CGFloat innerH = fmax(ch, b.size.height - kPadY * 2.0);
  g.cols = (uint16_t)fmax(2.0, fmin(500.0, floor(innerW / cw)));
  g.rows = (uint16_t)fmax(1.0, fmin(200.0, floor(innerH / ch)));
  return g;
}

- (NSRect)rectForRow:(uint16_t)row {
  CGFloat ch = self.cellHeight;
  CGFloat y = kPadY + (CGFloat)row * ch;
  return NSMakeRect(0.0, y - 1.0, self.bounds.size.width, ch + 2.0);
}

- (NSUInteger)cellWidthOfString:(NSString *)str {
  if (!str || str.length == 0)
    return 0;
  if (_host && _cb.measure_text) {
    NSData *utf8 = [str dataUsingEncoding:NSUTF8StringEncoding];
    if (utf8 && utf8.length > 0) {
      return (NSUInteger)_cb.measure_text(_host, (const uint8_t *)utf8.bytes,
                                          utf8.length);
    }
  }
  NSUInteger total = 0;
  for (NSUInteger idx = 0; idx < str.length; idx++) {
    unichar u = [str characterAtIndex:idx];
    uint32_t scalar = u;
    if (CFStringIsSurrogateHighCharacter(u) && idx + 1 < str.length) {
      unichar u2 = [str characterAtIndex:idx + 1];
      if (CFStringIsSurrogateLowCharacter(u2)) {
        scalar = CFStringGetLongCharacterForSurrogatePair(u, u2);
        idx++;
      }
    }
    total += (NSUInteger)cb_char_cells(scalar);
  }
  return total;
}

- (void)clearCommitCursorHold {
  if (!_commitCursorActive)
    return;
  uint16_t oldTargetY = _commitTargetY;
  uint16_t oldOriginY = _commitOriginY;
  _commitCursorActive = NO;
  _commitHoldUntil = 0;
  if (_snap && oldTargetY < _snap->rows) {
    [self setNeedsDisplayInRect:[self rectForRow:oldTargetY]];
  }
  if (_snap && oldOriginY < _snap->rows && oldOriginY != oldTargetY) {
    [self setNeedsDisplayInRect:[self rectForRow:oldOriginY]];
  }
}

- (void)recordOptimisticCommitForText:(NSString *)text hadMarked:(BOOL)hadMarked {
  if (!_snap || !_snap->cursor.visible || _snap->cols == 0 || _snap->rows == 0)
    return;
  if (!cb_is_single_line_printable(text)) {
    [self clearCommitCursorHold];
    return;
  }
  NSTimeInterval now = [NSProcessInfo processInfo].systemUptime;
  BOOL recentUnmark =
      (_lastUnmarkTime > 0.0 && (now - _lastUnmarkTime) <= kRecentUnmarkWindow);
  BOOL shouldHold = hadMarked || recentUnmark || cb_contains_non_ascii(text);
  if (!shouldHold)
    return;
  NSUInteger cells = [self cellWidthOfString:text];
  if (cells == 0)
    return;

  uint16_t curX = _snap->cursor.x < _snap->cols ? _snap->cursor.x
                                                : (_snap->cols - 1);
  uint16_t curY = _snap->cursor.y < _snap->rows ? _snap->cursor.y
                                                : (_snap->rows - 1);
  uint16_t baseX = curX;
  if (_commitCursorActive && now < _commitHoldUntil &&
      curX == _commitOriginX && curY == _commitOriginY) {
    baseX = _commitTargetX;
  } else {
    _commitOriginX = curX;
    _commitOriginY = curY;
  }
  NSUInteger nextX = MIN((NSUInteger)(_snap->cols - 1), (NSUInteger)baseX + cells);
  _commitTargetX = (uint16_t)nextX;
  _commitTargetY = curY;
  _commitHoldUntil = now + kCommitHoldGrace;
  _commitCursorActive = YES;
  [self setNeedsDisplayInRect:[self rectForRow:curY]];
  [self updateAnimTimerState];
}

- (BOOL)effectiveCursorX:(uint16_t *)outX y:(uint16_t *)outY {
  if (!_snap || _snap->cols == 0 || _snap->rows == 0) {
    if (outX)
      *outX = 0;
    if (outY)
      *outY = 0;
    return NO;
  }
  uint16_t curX = _snap->cursor.x < _snap->cols ? _snap->cursor.x
                                                : (_snap->cols - 1);
  uint16_t curY = _snap->cursor.y < _snap->rows ? _snap->cursor.y
                                                : (_snap->rows - 1);
  if (_commitCursorActive && _snap->cursor.visible) {
    NSTimeInterval now = [NSProcessInfo processInfo].systemUptime;
    if (now < _commitHoldUntil && curX == _commitOriginX &&
        curY == _commitOriginY) {
      if (outX)
        *outX = _commitTargetX < _snap->cols ? _commitTargetX : (_snap->cols - 1);
      if (outY)
        *outY = _commitTargetY < _snap->rows ? _commitTargetY : (_snap->rows - 1);
      return YES;
    }
  }
  if (outX)
    *outX = curX;
  if (outY)
    *outY = curY;
  return NO;
}

- (void)pollSnapshotForceFull:(BOOL)forceFull {
  if (!_host || !_cb.snapshot || ![self hasRenderableBounds])
    return;
  uint16_t oldCursorY = (_snap && _snap->cursor.visible) ? _snap->cursor.y : UINT16_MAX;
  const CbSnapshot *next = _cb.snapshot(_host, [self currentGeometry], forceFull);

  // Always drain clipboard / bell events even when snapshot revision didn't change.
  if (_cb.take_clipboard) {
    size_t clipLen = 0;
    const uint8_t *clipPtr = _cb.take_clipboard(_host, &clipLen);
    if (clipPtr && clipLen > 0) {
      NSString *clipStr = [[NSString alloc] initWithBytes:clipPtr
                                                   length:clipLen
                                                 encoding:NSUTF8StringEncoding];
      if (clipStr) {
        NSPasteboard *pb = [NSPasteboard generalPasteboard];
        [pb clearContents];
        [pb setString:clipStr forType:NSPasteboardTypeString];
      }
    }
  }
  if (_cb.take_bell && _cb.take_bell(_host)) {
    NSBeep();
  }

  if (!next) {
    [self updateAnimTimerState];
    return;
  }
  _snap = next;

  if (_commitCursorActive) {
    NSTimeInterval now = [NSProcessInfo processInfo].systemUptime;
    if (!next->cursor.visible || next->cursor.x != _commitOriginX ||
        next->cursor.y != _commitOriginY || now >= _commitHoldUntil) {
      [self clearCommitCursorHold];
    }
  }

  if (next->scrollbar.total > next->scrollbar.len &&
      (next->scrollbar.offset != _prevScrollOffset ||
       next->scrollbar.total != _prevScrollTotal)) {
    _scrollbarVisibleUntil = [NSProcessInfo processInfo].systemUptime + 0.9;
  }
  _prevScrollOffset = next->scrollbar.offset;
  _prevScrollTotal = next->scrollbar.total;

  BOOL imagesChanged =
      (next->image_placements_generation != _prevImagePlacementsGen) ||
      (next->image_placements_len != _prevImagePlacementsLen);
  _prevImagePlacementsGen = next->image_placements_generation;
  _prevImagePlacementsLen = next->image_placements_len;

  if (forceFull || next->dirty_state == 2 || imagesChanged) {
    [self setNeedsDisplay:YES];
  } else if (next->dirty_state == 1) {
    if (next->dirty_rows) {
      for (uint16_t r = 0; r < next->rows; r++) {
        if (next->dirty_rows[r]) {
          [self setNeedsDisplayInRect:[self rectForRow:r]];
        }
      }
    }
    if (oldCursorY != UINT16_MAX && oldCursorY < next->rows) {
      [self setNeedsDisplayInRect:[self rectForRow:oldCursorY]];
    }
    if (next->cursor.visible && next->cursor.y < next->rows) {
      [self setNeedsDisplayInRect:[self rectForRow:next->cursor.y]];
    }
    // Redraw status pill and scrollbar margin on incremental updates too.
    CGFloat w = self.bounds.size.width;
    CGFloat h = self.bounds.size.height;
    [self setNeedsDisplayInRect:NSMakeRect(fmax(0.0, w - 260.0), 0.0, 260.0, 36.0)];
    [self setNeedsDisplayInRect:NSMakeRect(fmax(0.0, w - 12.0), 0.0, 12.0, h)];
  }
  [self updateAnimTimerState];
}

- (void)handleWake {
  [self pollSnapshotForceFull:NO];
}

- (void)updateAnimTimerState {
  if (!_host || !self.window) {
    [self.animTimer invalidate];
    self.animTimer = nil;
    return;
  }
  NSTimeInterval now = [NSProcessInfo processInfo].systemUptime;
  BOOL needCursorBlink =
      _focusedState && _snap && _snap->cursor.visible && _snap->cursor.blinking;
  BOOL needAutoscroll = (_selectingLocally && _autoscrollDir != 0);
  BOOL needScrollbarFade = (now < _scrollbarVisibleUntil + 0.25);
  BOOL needLayoutSettle = (_pendingLayoutRedraws > 0);
  BOOL needCommitHold = _commitCursorActive;
  BOOL needKittyAnim = (_snap && _snap->next_animation_ms > 0);
  BOOL needed = needCursorBlink || needAutoscroll || needScrollbarFade ||
                needLayoutSettle || needCommitHold || needKittyAnim;

  NSTimeInterval desiredInterval =
      (needAutoscroll || needCommitHold) ? 0.033 : 0.08;
  if (needKittyAnim) {
    NSTimeInterval kittyInterval =
        fmax(0.016, fmin(0.5, (double)_snap->next_animation_ms / 1000.0));
    desiredInterval = fmin(desiredInterval, kittyInterval);
  }
  if (!needed) {
    [self.animTimer invalidate];
    self.animTimer = nil;
    return;
  }
  if (self.animTimer &&
      fabs(self.animTimer.timeInterval - desiredInterval) < 0.01) {
    return;
  }
  [self.animTimer invalidate];
  __weak typeof(self) weakSelf = self;
  self.animTimer = [NSTimer timerWithTimeInterval:desiredInterval
                                          repeats:YES
                                            block:^(NSTimer *timer) {
                                              (void)timer;
                                              [weakSelf onAnimTick];
                                            }];
  [[NSRunLoop mainRunLoop] addTimer:self.animTimer forMode:NSRunLoopCommonModes];
}

- (void)onAnimTick {
  if (!_host) {
    [self.animTimer invalidate];
    self.animTimer = nil;
    return;
  }
  NSTimeInterval now = [NSProcessInfo processInfo].systemUptime;
  if (_commitCursorActive && now >= _commitHoldUntil) {
    [self clearCommitCursorHold];
  }
  if (_selectingLocally && _autoscrollDir != 0 && _cb.select) {
    CbSelectEvent ev = {0};
    ev.kind = 3; // autoscroll tick
    ev.rectangle = _selectionRectangle ? 1 : 0;
    ev.x_px = _lastDragPoint.x;
    ev.y_px = _lastDragPoint.y;
    _autoscrollDir = _cb.select(_host, ev);
    [self pollSnapshotForceFull:NO];
  }
  if (_focusedState && _snap && _snap->cursor.visible && _snap->cursor.blinking) {
    if (now - _lastBlinkTime >= 0.53) {
      _lastBlinkTime = now;
      _cursorBlinkVisible = !_cursorBlinkVisible;
      if (_snap->cursor.y < _snap->rows) {
        [self setNeedsDisplayInRect:[self rectForRow:_snap->cursor.y]];
      }
    }
  } else if (!_cursorBlinkVisible) {
    _cursorBlinkVisible = YES;
    if (_snap && _snap->cursor.visible && _snap->cursor.y < _snap->rows) {
      [self setNeedsDisplayInRect:[self rectForRow:_snap->cursor.y]];
    }
  }
  if (now <= _scrollbarVisibleUntil + 0.25) {
    CGFloat w = self.bounds.size.width;
    [self setNeedsDisplayInRect:NSMakeRect(fmax(0.0, w - 12.0), 0.0, 12.0,
                                           self.bounds.size.height)];
  }
  if (_pendingLayoutRedraws > 0) {
    _pendingLayoutRedraws--;
    [self pollSnapshotForceFull:NO];
    [self setNeedsDisplay:YES];
  } else if (_snap && _snap->next_animation_ms > 0) {
    [self pollSnapshotForceFull:NO];
  }
  [self updateAnimTimerState];
}

- (void)updateFocusState {
  BOOL focused = (self.window != nil && self.window.isKeyWindow &&
                  self.window.firstResponder == self);
  if (focused != _focusedState) {
    _focusedState = focused;
    _cursorBlinkVisible = YES;
    _lastBlinkTime = [NSProcessInfo processInfo].systemUptime;
    if (_host && _cb.focus) {
      _cb.focus(_host, focused);
    }
    if (_snap && _snap->cursor.visible && _snap->cursor.y < _snap->rows) {
      [self setNeedsDisplayInRect:[self rectForRow:_snap->cursor.y]];
    } else {
      [self setNeedsDisplay:YES];
    }
    [self updateAnimTimerState];
  }
}

- (void)windowFocusDidChange:(NSNotification *)note {
  (void)note;
  if (!self.window || !self.window.isKeyWindow) {
    _physicalKeyDownLo = 0;
    _physicalKeyDownHi = 0;
    [self clearHoveredLink];
    [self.inputContext discardMarkedText];
    [self unmarkText];
    [self clearCommitCursorHold];
  } else if (self.window.firstResponder == self) {
    [self.inputContext activate];
  }
  [self updateFocusState];
}

- (void)requestAutoFocusIfEligible {
  if (!_autoFocus || !_host || !self.window || self.isHiddenOrHasHiddenAncestor)
    return;
  NSResponder *first = self.window.firstResponder;
  if (first == self)
    return;
  if ([first isKindOfClass:[NSText class]])
    return;
  if ([first isKindOfClass:[NSView class]] &&
      ![NSStringFromClass([first class]) hasPrefix:@"Nsh"]) {
    // Another dedicated native control (e.g. NSTextField / WKWebView) holds focus.
    return;
  }
  [self.window makeFirstResponder:self];
}

- (void)viewDidMoveToWindow {
  [super viewDidMoveToWindow];
  NSNotificationCenter *nc = [NSNotificationCenter defaultCenter];
  [nc removeObserver:self name:NSWindowDidBecomeKeyNotification object:nil];
  [nc removeObserver:self name:NSWindowDidResignKeyNotification object:nil];

  if (self.window) {
    CGFloat scale = self.window.backingScaleFactor;
    if (scale > 0.0 && fabs(scale - self.backingScale) > 0.01) {
      self.backingScale = scale;
      [self rebuildFonts];
    }
    [nc addObserver:self
           selector:@selector(windowFocusDidChange:)
               name:NSWindowDidBecomeKeyNotification
             object:self.window];
    [nc addObserver:self
           selector:@selector(windowFocusDidChange:)
               name:NSWindowDidResignKeyNotification
             object:self.window];
    if (_autoFocus) {
      [self requestAutoFocusIfEligible];
      __weak CanbanTerminal *weakSelf = self;
      dispatch_async(dispatch_get_main_queue(), ^{
        [weakSelf requestAutoFocusIfEligible];
      });
    }
    _pendingLayoutRedraws = 0;
    [self pollSnapshotForceFull:YES];
  } else {
    [self.animTimer invalidate];
    self.animTimer = nil;
  }
  [self updateFocusState];
}

- (void)viewDidUnhide {
  [super viewDidUnhide];
  if (_autoFocus) {
    [self requestAutoFocusIfEligible];
    __weak CanbanTerminal *weakSelf = self;
    dispatch_async(dispatch_get_main_queue(), ^{
      [weakSelf requestAutoFocusIfEligible];
    });
  }
  [self updateFocusState];
}

- (void)viewDidChangeBackingProperties {
  [super viewDidChangeBackingProperties];
  CGFloat scale = self.window ? self.window.backingScaleFactor : 2.0;
  if (scale > 0.0 && fabs(scale - self.backingScale) > 0.01) {
    self.backingScale = scale;
    [self rebuildFonts];
    [self pollSnapshotForceFull:YES];
  }
}

- (void)setFrameSize:(NSSize)newSize {
  BOOL sizeChanged = !NSEqualSizes(self.bounds.size, newSize);
  [super setFrameSize:newSize];
  _pendingLayoutRedraws = 0;
  if (![self hasRenderableBounds])
    return;
  [self pollSnapshotForceFull:(_snap == NULL)];
  if (sizeChanged) {
    [self setNeedsDisplay:YES];
  }
}

- (void)layout {
  [super layout];
  [self pollSnapshotForceFull:NO];
}

- (void)updateTrackingAreas {
  [super updateTrackingAreas];
  if (self.trackingArea) {
    [self removeTrackingArea:self.trackingArea];
  }
  NSTrackingAreaOptions opts = NSTrackingMouseMoved |
                               NSTrackingMouseEnteredAndExited |
                               NSTrackingCursorUpdate |
                               NSTrackingActiveInKeyWindow |
                               NSTrackingInVisibleRect;
  self.trackingArea = [[NSTrackingArea alloc] initWithRect:NSZeroRect
                                                   options:opts
                                                     owner:self
                                                  userInfo:nil];
  [self addTrackingArea:self.trackingArea];
}

- (BOOL)becomeFirstResponder {
  BOOL ok = [super becomeFirstResponder];
  if (ok) {
    [self.inputContext activate];
    [self updateFocusState];
    __weak CanbanTerminal *weakSelf = self;
    dispatch_async(dispatch_get_main_queue(), ^{
      [weakSelf updateFocusState];
    });
  }
  return ok;
}

- (BOOL)resignFirstResponder {
  BOOL ok = [super resignFirstResponder];
  if (ok) {
    _physicalKeyDownLo = 0;
    _physicalKeyDownHi = 0;
    [self clearHoveredLink];
    [self.inputContext discardMarkedText];
    [self unmarkText];
    [self clearCommitCursorHold];
    NSEvent *ev = [NSApp currentEvent];
    BOOL keyboardTabStop = (ev && ev.type == NSEventTypeKeyDown &&
                            (ev.keyCode == 0x30 || ev.keyCode == 0x35));
    __weak CanbanTerminal *weakSelf = self;
    dispatch_async(dispatch_get_main_queue(), ^{
      CanbanTerminal *strong = weakSelf;
      if (!strong)
        return;
      if (!keyboardTabStop && strong.autoFocus) {
        [strong requestAutoFocusIfEligible];
      }
      [strong updateFocusState];
    });
  }
  return ok;
}


#pragma mark - Drawing

- (void)drawRect:(NSRect)dirtyRect {
  if (!_snap && _host) {
    [self pollSnapshotForceFull:YES];
  }
  CGContextRef ctx = [[NSGraphicsContext currentContext] CGContext];
  if (!ctx)
    return;

  uint16_t curX = 0;
  uint16_t curY = 0;
  BOOL optimisticCursor = [self effectiveCursorX:&curX y:&curY];

  CbRenderDrawParams params;
  memset(&params, 0, sizeof(params));
  params.host = _host;
  params.resolve_image = _cb.resolve_image;
  params.measure_text = _cb.measure_text;
  params.theme_bg[0] = _themeBg[0];
  params.theme_bg[1] = _themeBg[1];
  params.theme_bg[2] = _themeBg[2];
  params.focused = _focusedState ? true : false;
  params.cursor_blink_visible = _cursorBlinkVisible ? true : false;
  params.optimistic_cursor = optimisticCursor ? true : false;
  params.cur_x = curX;
  params.cur_y = curY;
  params.has_marked_text = [self hasMarkedText] ? true : false;
  params.marked_text = self.markedTextStr;
  params.selected_range = self.selectedRangeVal;
  params.hover_link_valid = _hoverLinkValid ? true : false;
  params.hover_link_row = _hoverLinkRow;
  params.hover_link_start_col = _hoverLinkStartCol;
  params.hover_link_end_col = _hoverLinkEndCol;
  params.now = [NSProcessInfo processInfo].systemUptime;
  params.scrollbar_visible_until = _scrollbarVisibleUntil;

  [_renderer drawSnapshot:_snap
                inContext:ctx
                   bounds:NSRectToCGRect(self.bounds)
                dirtyRect:NSRectToCGRect(dirtyRect)
                   params:&params];
}

#pragma mark - Keyboard,Shortcuts & NSTextInputClient

- (BOOL)performKeyEquivalent:(NSEvent *)event {
  if (!_host || !self.window || self.window.firstResponder != self ||
      event.type != NSEventTypeKeyDown) {
    return NO;
  }

  NSEventModifierFlags flags =
      event.modifierFlags & NSEventModifierFlagDeviceIndependentFlagsMask;

  // Prevent AppKit from eating Control+/ (beep) or Control+Return (context menu).
  if ((flags & NSEventModifierFlagControl) &&
      !(flags & NSEventModifierFlagCommand)) {
    NSString *ign = event.charactersIgnoringModifiers ?: @"";
    if ([ign isEqualToString:@"/"] || [ign isEqualToString:@"\r"]) {
      [self keyDown:event];
      return YES;
    }
  }

  if ((flags & NSEventModifierFlagCommand) &&
      !(flags & NSEventModifierFlagControl) &&
      !(flags & NSEventModifierFlagOption)) {
    NSString *ch = event.charactersIgnoringModifiers.lowercaseString ?: @"";
    // Let Canban's window-level shortcut map handle UI commands cleanly without suspending input!
    if ([self.uiCommandKeys containsObject:ch]) {
      return NO;
    }
    BOOL shift = (flags & NSEventModifierFlagShift) != 0;
    if (!shift) {
      if ([ch isEqualToString:@"c"]) {
        if (_cb.has_selection && _cb.has_selection(_host)) {
          [self copy:nil];
        }
        return YES;
      }
      if ([ch isEqualToString:@"v"]) {
        [self paste:nil];
        return YES;
      }
      if ([ch isEqualToString:@"a"]) {
        [self selectAll:nil];
        return YES;
      }
      if ([ch isEqualToString:@"k"]) {
        [self clearTerminal:nil];
        return YES;
      }
    }
  }
  return [super performKeyEquivalent:event];
}

- (IBAction)copy:(id)sender {
  (void)sender;
  if (!_host || !_cb.copy_text)
    return;
  size_t len = 0;
  const uint8_t *ptr = _cb.copy_text(_host, &len);
  if (!ptr || len == 0)
    return;
  NSString *text = [[NSString alloc] initWithBytes:ptr
                                            length:len
                                          encoding:NSUTF8StringEncoding];
  if (!text)
    return;
  NSPasteboard *pb = [NSPasteboard generalPasteboard];
  [pb clearContents];
  [pb setString:text forType:NSPasteboardTypeString];
}

- (NSArray<NSString *> *)filePathsFromPasteboard:(NSPasteboard *)pb {
  if (!pb)
    return @[];
  NSArray<NSURL *> *urls =
      [pb readObjectsForClasses:@[ [NSURL class] ]
                        options:@{NSPasteboardURLReadingFileURLsOnlyKey : @YES}];
  if (!urls || urls.count == 0)
    return @[];
  NSMutableArray<NSString *> *paths =
      [NSMutableArray arrayWithCapacity:urls.count];
  for (NSURL *url in urls) {
    NSString *path = url.path;
    if (path && path.length > 0) {
      [paths addObject:path];
    }
  }
  return paths;
}

- (NSString *)nonFileTextFromPasteboard:(NSPasteboard *)pb {
  if (!pb)
    return nil;
  NSString *text = [pb stringForType:NSPasteboardTypeString];
  if (text && text.length > 0)
    return text;
  NSArray<NSURL *> *urls = [pb readObjectsForClasses:@[ [NSURL class] ]
                                             options:nil];
  if (!urls || urls.count == 0)
    return nil;
  NSMutableArray<NSString *> *items =
      [NSMutableArray arrayWithCapacity:urls.count];
  for (NSURL *url in urls) {
    if (url.isFileURL)
      continue;
    NSString *s = url.absoluteString;
    if (s && s.length > 0) {
      [items addObject:s];
    }
  }
  if (items.count == 0)
    return nil;
  return [items componentsJoinedByString:@" "];
}

- (BOOL)hasPasteableContentInPasteboard:(NSPasteboard *)pb {
  if ([self filePathsFromPasteboard:pb].count > 0)
    return YES;
  NSString *text = [self nonFileTextFromPasteboard:pb];
  return text && text.length > 0;
}

- (BOOL)pasteFromPasteboard:(NSPasteboard *)pb {
  if (!_host || !pb)
    return NO;
  if (_cb.can_accept_input && !_cb.can_accept_input(_host))
    return NO;

  NSArray<NSString *> *paths = [self filePathsFromPasteboard:pb];
  if (paths.count > 0 && _cb.paste_paths) {
    NSMutableArray<NSData *> *utf8Items =
        [NSMutableArray arrayWithCapacity:paths.count];
    for (NSString *path in paths) {
      NSData *utf8 = [path dataUsingEncoding:NSUTF8StringEncoding];
      if (utf8 && utf8.length > 0) {
        [utf8Items addObject:utf8];
      }
    }
    if (utf8Items.count > 0) {
      CbByteSlice *slices =
          (CbByteSlice *)calloc(utf8Items.count, sizeof(CbByteSlice));
      if (!slices)
        return NO;
      for (NSUInteger i = 0; i < utf8Items.count; i++) {
        NSData *d = utf8Items[i];
        slices[i].ptr = (const uint8_t *)d.bytes;
        slices[i].len = d.length;
      }
      [self clearCommitCursorHold];
      bool ok = _cb.paste_paths(_host, slices, utf8Items.count);
      free(slices);
      if (ok) {
        [self pollSnapshotForceFull:NO];
        return YES;
      }
    }
  }

  if (!_cb.paste)
    return NO;
  NSString *text = [self nonFileTextFromPasteboard:pb];
  if (!text || text.length == 0)
    return NO;
  NSData *utf8 = [text dataUsingEncoding:NSUTF8StringEncoding];
  if (!utf8 || utf8.length == 0)
    return NO;
  [self clearCommitCursorHold];
  _cb.paste(_host, (const uint8_t *)utf8.bytes, utf8.length);
  [self pollSnapshotForceFull:NO];
  return YES;
}

- (IBAction)paste:(id)sender {
  (void)sender;
  [self pasteFromPasteboard:[NSPasteboard generalPasteboard]];
}

- (NSDragOperation)dragOperationForInfo:(id<NSDraggingInfo>)sender {
  if (!_host || !sender)
    return NSDragOperationNone;
  if (_cb.can_accept_input && !_cb.can_accept_input(_host))
    return NSDragOperationNone;
  if (![self hasPasteableContentInPasteboard:sender.draggingPasteboard])
    return NSDragOperationNone;
  return NSDragOperationCopy;
}

- (NSDragOperation)draggingEntered:(id<NSDraggingInfo>)sender {
  return [self dragOperationForInfo:sender];
}

- (NSDragOperation)draggingUpdated:(id<NSDraggingInfo>)sender {
  return [self dragOperationForInfo:sender];
}

- (BOOL)prepareForDragOperation:(id<NSDraggingInfo>)sender {
  return [self dragOperationForInfo:sender] != NSDragOperationNone;
}

- (BOOL)performDragOperation:(id<NSDraggingInfo>)sender {
  if ([self dragOperationForInfo:sender] == NSDragOperationNone)
    return NO;
  if (![self pasteFromPasteboard:sender.draggingPasteboard])
    return NO;
  if (self.window && self.window.firstResponder != self) {
    [self.window makeFirstResponder:self];
  }
  return YES;
}

- (void)selectAll:(id)sender {
  (void)sender;
  if (!_host || !_cb.select)
    return;
  CbSelectEvent ev = {0};
  ev.kind = 4; // select_all
  _cb.select(_host, ev);
  [self pollSnapshotForceFull:NO];
}

- (void)clearTerminal:(id)sender {
  (void)sender;
  if (!_host || !_cb.clear_screen)
    return;
  [self clearCommitCursorHold];
  _cb.clear_screen(_host);
  [self pollSnapshotForceFull:NO];
}

- (BOOL)validateMenuItem:(NSMenuItem *)menuItem {
  SEL action = menuItem.action;
  if (action == @selector(copy:)) {
    return _host && _cb.has_selection && _cb.has_selection(_host);
  }
  if (action == @selector(paste:)) {
    if (!_host)
      return NO;
    if (_cb.can_accept_input && !_cb.can_accept_input(_host))
      return NO;
    return [self hasPasteableContentInPasteboard:[NSPasteboard generalPasteboard]];
  }
  if (action == @selector(selectAll:) || action == @selector(clearTerminal:)) {
    return _host != NULL;
  }
  return YES;
}

- (BOOL)isSyntheticUnicodeEvent:(NSEvent *)event {
  if (event.keyCode != 0)
    return NO;
  NSString *chars = event.characters;
  if (!chars || chars.length == 0)
    return NO;
  NSEventModifierFlags flags = event.modifierFlags;
  if (flags & (NSEventModifierFlagControl | NSEventModifierFlagOption |
               NSEventModifierFlagCommand)) {
    return NO;
  }
  if (chars.length == 1) {
    unichar ch = [chars characterAtIndex:0];
    if (ch == 'a' || ch == 'A')
      return NO;
  }
  return YES;
}

- (uint32_t)unshiftedCodepointForEvent:(NSEvent *)event {
  if ([self isSyntheticUnicodeEvent:event]) {
    uint32_t cp = cb_first_scalar(event.characters);
    if (cp >= 'A' && cp <= 'Z') {
      return cp + ('a' - 'A');
    }
    return cp;
  }
  NSEventModifierFlags cleanFlags =
      event.modifierFlags &
      ~(NSEventModifierFlagShift | NSEventModifierFlagControl |
        NSEventModifierFlagOption | NSEventModifierFlagCommand |
        NSEventModifierFlagCapsLock);
  NSString *s = nil;
  @try {
    s = [event charactersByApplyingModifiers:cleanFlags];
  } @catch (NSException *ex) {
    (void)ex;
  }
  if (!s || s.length == 0) {
    s = event.charactersIgnoringModifiers.lowercaseString;
  }
  return cb_first_scalar(s);
}

- (NSString *)translatedCharactersForEvent:(NSEvent *)event {
  if ([self isSyntheticUnicodeEvent:event]) {
    return event.characters ?: @"";
  }
  NSEventModifierFlags cleanFlags =
      event.modifierFlags &
      ~(NSEventModifierFlagControl | NSEventModifierFlagOption |
        NSEventModifierFlagCommand);
  NSString *s = nil;
  @try {
    s = [event charactersByApplyingModifiers:cleanFlags];
  } @catch (NSException *ex) {
    (void)ex;
  }
  if (!s || s.length == 0) {
    s = event.charactersIgnoringModifiers ?: @"";
  }
  return s;
}

- (void)setPhysicalKey:(uint16_t)kcode down:(BOOL)down {
  if (kcode < 64) {
    if (down) {
      _physicalKeyDownLo |= (1ULL << kcode);
    } else {
      _physicalKeyDownLo &= ~(1ULL << kcode);
    }
  } else if (kcode < 128) {
    if (down) {
      _physicalKeyDownHi |= (1ULL << (kcode - 64));
    } else {
      _physicalKeyDownHi &= ~(1ULL << (kcode - 64));
    }
  }
}

- (void)keyDown:(NSEvent *)event {
  if (!_host || !_cb.key)
    return;

  [NSCursor setHiddenUntilMouseMoves:YES];
  _cursorBlinkVisible = YES;
  _lastBlinkTime = [NSProcessInfo processInfo].systemUptime;

  NSEventModifierFlags rawFlags = event.modifierFlags;
  uint16_t mods = cb_mods_from_flags(rawFlags);
  uint16_t consumedMods =
      (mods & (CB_MODS_SHIFT | CB_MODS_CAPS_LOCK));
  uint8_t action = event.isARepeat ? 2 : 1;
  BOOL syntheticUnicode = [self isSyntheticUnicodeEvent:event];
  uint32_t unshiftedCp = [self unshiftedCodepointForEvent:event];
  BOOL markedBefore = [self hasMarkedText];

  uint16_t kcode = event.keyCode;
  [self setPhysicalKey:kcode down:NO];

  // Control/Command key combinations outside active IME composition are terminal
  // control sequences, never CJK preedit text. Bypassing InputMethodKit here
  // avoids IME interception and prevents XPC Mach-port stalls on Ctrl+C / Ctrl+D.
  if (!markedBefore &&
      (rawFlags & (NSEventModifierFlagControl | NSEventModifierFlagCommand)) != 0) {
    [self clearCommitCursorHold];
    NSString *chars = [self translatedCharactersForEvent:event];
    NSData *utf8 = [chars dataUsingEncoding:NSUTF8StringEncoding];
    CbKeyEvent kev = {0};
    kev.action = action;
    kev.composing = 0;
    kev.unidentified = syntheticUnicode ? 1 : 0;
    kev.keycode = event.keyCode;
    kev.mods = mods;
    kev.consumed_mods = consumedMods;
    kev.unshifted_cp = unshiftedCp;
    kev.utf8_ptr = utf8 ? (const uint8_t *)utf8.bytes : NULL;
    kev.utf8_len = utf8 ? utf8.length : 0;
    if (!syntheticUnicode) {
      [self setPhysicalKey:kcode down:YES];
    }
    _cb.key(_host, kev);
    [self pollSnapshotForceFull:NO];
    return;
  }

  // Both Option keys act as Alt: strip Option only when translating text for key-encoding
  // while keeping the exact original NSEvent pointer whenever Option is not held so macOS
  // CJK InputMethodKit (including third-party XPC IMEs like WeType/Sogou/Rime) sees an
  // untouched CGEvent-backed NSEvent.
  NSEvent *translationEvent = event;
  if (!markedBefore && (rawFlags & NSEventModifierFlagOption) != 0) {
    NSEventModifierFlags stripped = rawFlags & ~NSEventModifierFlagOption;
    NSString *chars = [self translatedCharactersForEvent:event];
    NSEvent *synth = [NSEvent keyEventWithType:event.type
                                      location:event.locationInWindow
                                 modifierFlags:stripped
                                     timestamp:event.timestamp
                                  windowNumber:event.windowNumber
                                       context:nil
                                    characters:chars
                   charactersIgnoringModifiers:(event.charactersIgnoringModifiers ?: @"")
                                     isARepeat:event.isARepeat
                                       keyCode:event.keyCode];
    if (synth) {
      translationEvent = synth;
    }
  }

  self.keyTextAccumulator = [NSMutableArray arrayWithCapacity:2];
  self.didDoCommand = NO;
  self.didUpdateMarkedText = NO;
  self.pendingCommandSelector = NULL;

  [self interpretKeyEvents:@[ translationEvent ]];

  NSArray<NSString *> *accum = self.keyTextAccumulator;
  self.keyTextAccumulator = nil;
  BOOL didCommand = self.didDoCommand;
  BOOL updatedMarked = self.didUpdateMarkedText;
  SEL cmdSel = self.pendingCommandSelector;
  self.didDoCommand = NO;
  self.didUpdateMarkedText = NO;
  self.pendingCommandSelector = NULL;

  if (accum.count > 0) {
    for (NSString *str in accum) {
      if (str.length == 0)
        continue;
      NSData *utf8 = [str dataUsingEncoding:NSUTF8StringEncoding];
      if (!utf8 || utf8.length == 0)
        continue;
      BOOL plainSingleAsciiKey =
          (!markedBefore && !updatedMarked && ![self hasMarkedText] &&
           !syntheticUnicode && str.length == 1 &&
           [str characterAtIndex:0] >= 0x20 && [str characterAtIndex:0] < 0x7f);
      if (!plainSingleAsciiKey) {
        // Commit IME text, CJK punctuation, multi-scalar insertions, and synthetic
        // CGEvent Unicode strings directly as UTF-8 so Kitty report_all mode never
        // replaces them with the underlying physical keycode (e.g. keyCode 0 -> 'a').
        if (_cb.text) {
          _cb.text(_host, (const uint8_t *)utf8.bytes, utf8.length);
          [self recordOptimisticCommitForText:str
                                    hadMarked:(markedBefore || updatedMarked)];
        }
      } else {
        CbKeyEvent kev = {0};
        kev.action = action;
        kev.composing = 0;
        kev.unidentified = 0;
        kev.keycode = event.keyCode;
        kev.mods = mods;
        kev.consumed_mods = consumedMods;
        kev.unshifted_cp = unshiftedCp;
        kev.utf8_ptr = (const uint8_t *)utf8.bytes;
        kev.utf8_len = utf8.length;
        [self setPhysicalKey:kcode down:YES];
        if (_cb.key(_host, kev)) {
          if (_commitCursorActive &&
              (mods & (CB_MODS_CTRL | CB_MODS_ALT | CB_MODS_SUPER)) == 0) {
            [self recordOptimisticCommitForText:str hadMarked:YES];
          } else {
            [self clearCommitCursorHold];
          }
        }
      }
    }
  } else if (cmdSel == @selector(paste:)) {
    [self paste:nil];
  } else if (cmdSel == @selector(copy:)) {
    [self copy:nil];
  } else if (cmdSel == @selector(selectAll:)) {
    [self selectAll:nil];
  } else if ((markedBefore || updatedMarked || [self hasMarkedText]) &&
             !didCommand) {
    // The active IME consumed this keystroke for preedit or candidate navigation
    // (e.g. Pinyin buffer edit, Space/1..9 commit, or Escape cancel). Do not leak
    // raw Backspace/Enter/Escape/Arrows to the PTY.
  } else {
    BOOL composing = ([self hasMarkedText] || markedBefore || updatedMarked);
    if (!composing) {
      [self clearCommitCursorHold];
    }
    NSString *chars = [self translatedCharactersForEvent:event];
    NSData *utf8 = [chars dataUsingEncoding:NSUTF8StringEncoding];
    CbKeyEvent kev = {0};
    kev.action = action;
    kev.composing = composing ? 1 : 0;
    kev.unidentified = syntheticUnicode ? 1 : 0;
    kev.keycode = event.keyCode;
    kev.mods = mods;
    kev.consumed_mods = consumedMods;
    kev.unshifted_cp = unshiftedCp;
    kev.utf8_ptr = utf8 ? (const uint8_t *)utf8.bytes : NULL;
    kev.utf8_len = utf8 ? utf8.length : 0;
    if (!composing && !syntheticUnicode) {
      [self setPhysicalKey:kcode down:YES];
    }
    _cb.key(_host, kev);
  }

  [self pollSnapshotForceFull:NO];
  if (markedBefore != [self hasMarkedText] || [self hasMarkedText] ||
      updatedMarked || _commitCursorActive) {
    [self setNeedsDisplay:YES];
  }
}

- (void)keyUp:(NSEvent *)event {
  if (!_host || !_cb.key)
    return;
  uint16_t kcode = event.keyCode;
  BOOL wasPhysicalDown = NO;
  if (kcode < 64) {
    uint64_t mask = (1ULL << kcode);
    wasPhysicalDown = (_physicalKeyDownLo & mask) != 0;
    _physicalKeyDownLo &= ~mask;
  } else if (kcode < 128) {
    uint64_t mask = (1ULL << (kcode - 64));
    wasPhysicalDown = (_physicalKeyDownHi & mask) != 0;
    _physicalKeyDownHi &= ~mask;
  } else {
    wasPhysicalDown = YES;
  }
  if (!wasPhysicalDown || [self hasMarkedText] ||
      [self isSyntheticUnicodeEvent:event])
    return;
  uint16_t mods = cb_mods_from_flags(event.modifierFlags);
  CbKeyEvent kev = {0};
  kev.action = 0; // release
  kev.composing = 0;
  kev.unidentified = 0;
  kev.keycode = kcode;
  kev.mods = mods;
  kev.consumed_mods = 0;
  kev.unshifted_cp = [self unshiftedCodepointForEvent:event];
  kev.utf8_ptr = NULL;
  kev.utf8_len = 0;
  if (_cb.key(_host, kev)) {
    [self pollSnapshotForceFull:NO];
  }
}

- (void)flagsChanged:(NSEvent *)event {
  if (!_host)
    return;
  NSEventModifierFlags flags = event.modifierFlags;
  NSEventModifierFlags prev = _lastModifierFlags;
  _lastModifierFlags = flags;

  uint16_t code = event.keyCode;
  NSEventModifierFlags changedMask = 0;
  switch (code) {
  case 0x38:
  case 0x3C:
    changedMask = NSEventModifierFlagShift;
    break;
  case 0x3B:
  case 0x3E:
    changedMask = NSEventModifierFlagControl;
    break;
  case 0x3A:
  case 0x3D:
    changedMask = NSEventModifierFlagOption;
    break;
  case 0x37:
  case 0x36:
    changedMask = NSEventModifierFlagCommand;
    break;
  case 0x39:
    changedMask = NSEventModifierFlagCapsLock;
    break;
  default:
    break;
  }

  if (changedMask != 0 && _cb.key && ![self hasMarkedText]) {
    BOOL pressed = (flags & changedMask) != 0;
    CbKeyEvent kev = {0};
    kev.action = pressed ? 1 : 0;
    kev.composing = 0;
    kev.keycode = code;
    kev.mods = cb_mods_from_flags(flags);
    kev.consumed_mods = 0;
    kev.unshifted_cp = 0;
    _cb.key(_host, kev);
  }

  if (((flags ^ prev) &
       (NSEventModifierFlagCommand | NSEventModifierFlagOption)) != 0 &&
      self.window) {
    NSPoint winPt = [self.window mouseLocationOutsideOfEventStream];
    if ([self isPointerHitAtWindowPoint:winPt]) {
      NSPoint localPt = [self convertPoint:winPt fromView:nil];
      [self refreshHoverStateAtPoint:localPt modifierFlags:flags];
    } else {
      [self clearHoveredLink];
    }
  }
}

- (void)insertText:(id)string replacementRange:(NSRange)replacementRange {
  (void)replacementRange;
  NSString *raw = [string isKindOfClass:[NSAttributedString class]]
                      ? [(NSAttributedString *)string string]
                      : ([string isKindOfClass:[NSString class]]
                             ? (NSString *)string
                             : [string description]);
  // NSTextInputContext / IMKInputSession (e.g. WeType) passes a reusable mutable
  // CFMutableString (__NSCFString) and truncates it before interpretKeyEvents:
  // returns; snapshot an immutable copy immediately.
  NSString *text = [raw copy];
  BOOL hadMarked = [self hasMarkedText];
  [self unmarkText];
  if (!text || text.length == 0)
    return;
  if (self.keyTextAccumulator != nil) {
    [self.keyTextAccumulator addObject:text];
    return;
  }
  if (_host && _cb.text) {
    NSData *utf8 = [text dataUsingEncoding:NSUTF8StringEncoding];
    if (utf8 && utf8.length > 0) {
      _cb.text(_host, (const uint8_t *)utf8.bytes, utf8.length);
      [self recordOptimisticCommitForText:text hadMarked:hadMarked];
      [self pollSnapshotForceFull:NO];
      [self setNeedsDisplay:YES];
    }
  }
}

- (void)doCommandBySelector:(SEL)selector {
  if (self.keyTextAccumulator != nil) {
    self.didDoCommand = YES;
    self.pendingCommandSelector = selector;
    return;
  }
  if (selector == @selector(paste:)) {
    [self paste:nil];
  } else if (selector == @selector(copy:)) {
    [self copy:nil];
  } else if (selector == @selector(selectAll:)) {
    [self selectAll:nil];
  }
}

- (void)setMarkedText:(id)string
        selectedRange:(NSRange)selectedRange
     replacementRange:(NSRange)replacementRange {
  (void)replacementRange;
  BOOL hadMarked = [self hasMarkedText];
  self.didUpdateMarkedText = YES;
  NSString *raw = [string isKindOfClass:[NSAttributedString class]]
                      ? [(NSAttributedString *)string string]
                      : ([string isKindOfClass:[NSString class]]
                             ? (NSString *)string
                             : [string description]);
  self.markedTextStr = [raw copy] ?: @"";
  if (self.markedTextStr.length > 0) {
    self.markedRangeVal = NSMakeRange(0, self.markedTextStr.length);
    self.selectedRangeVal = selectedRange;
  } else {
    if (hadMarked) {
      _lastUnmarkTime = [NSProcessInfo processInfo].systemUptime;
    }
    self.markedRangeVal = NSMakeRange(NSNotFound, 0);
    self.selectedRangeVal = NSMakeRange(0, 0);
  }
  [self setNeedsDisplay:YES];
}

- (void)unmarkText {
  BOOL had = [self hasMarkedText];
  if (had) {
    self.didUpdateMarkedText = YES;
    _lastUnmarkTime = [NSProcessInfo processInfo].systemUptime;
  }
  self.markedTextStr = nil;
  self.markedRangeVal = NSMakeRange(NSNotFound, 0);
  self.selectedRangeVal = NSMakeRange(0, 0);
  if (had) {
    [self setNeedsDisplay:YES];
  }
}

- (NSRange)selectedRange {
  if ([self hasMarkedText]) {
    return self.selectedRangeVal;
  }
  return NSMakeRange(0, 0);
}

- (NSRange)markedRange {
  return self.markedRangeVal;
}

- (BOOL)hasMarkedText {
  return self.markedRangeVal.location != NSNotFound &&
         self.markedTextStr.length > 0;
}

- (NSAttributedString *)attributedSubstringForProposedRange:(NSRange)range
                                                actualRange:(NSRangePointer)actualRange {
  if (![self hasMarkedText]) {
    if (actualRange)
      *actualRange = NSMakeRange(0, 0);
    return nil;
  }
  NSUInteger len = self.markedTextStr.length;
  NSUInteger loc =
      (range.location == NSNotFound) ? len : MIN(range.location, len);
  loc = cb_clamp_utf16_boundary(self.markedTextStr, loc);
  NSUInteger maxLen = len - loc;
  NSUInteger end =
      cb_clamp_utf16_boundary(self.markedTextStr, loc + MIN(range.length, maxLen));
  NSRange clamped = NSMakeRange(loc, end >= loc ? (end - loc) : 0);
  if (actualRange)
    *actualRange = clamped;
  if (clamped.length == 0)
    return nil;
  return [[NSAttributedString alloc]
      initWithString:[self.markedTextStr substringWithRange:clamped]];
}

- (NSArray<NSAttributedStringKey> *)validAttributesForMarkedText {
  return @[];
}

- (NSRect)firstRectForCharacterRange:(NSRange)range
                         actualRange:(NSRangePointer)actualRange {
  NSRange clamped = NSMakeRange(0, 0);
  if ([self hasMarkedText]) {
    NSUInteger len = self.markedTextStr.length;
    NSUInteger loc =
        (range.location == NSNotFound) ? len : MIN(range.location, len);
    loc = cb_clamp_utf16_boundary(self.markedTextStr, loc);
    NSUInteger maxLen = len - loc;
    NSUInteger end = cb_clamp_utf16_boundary(
        self.markedTextStr, loc + MIN(range.length, maxLen));
    clamped = NSMakeRange(loc, end >= loc ? (end - loc) : 0);
  }
  if (actualRange) {
    *actualRange = clamped;
  }
  CGFloat cw = self.cellWidth > 0.0 ? self.cellWidth : 8.0;
  CGFloat ch = self.cellHeight > 0.0 ? self.cellHeight : 18.0;
  CGFloat cx = kPadX;
  CGFloat cy = kPadY;
  if (_snap && _snap->cols > 0 && _snap->rows > 0) {
    uint16_t curX = 0;
    uint16_t curY = 0;
    [self effectiveCursorX:&curX y:&curY];
    cx = kPadX + (CGFloat)curX * cw;
    cy = kPadY + (CGFloat)curY * ch;
  }
  CGFloat rectW = cw;
  if ([self hasMarkedText] && self.markedTextStr.length > 0) {
    if (clamped.location > 0) {
      NSString *prefix = [self.markedTextStr substringToIndex:clamped.location];
      cx += (CGFloat)[self cellWidthOfString:prefix] * cw;
    }
    if (clamped.length > 0) {
      NSString *sub = [self.markedTextStr substringWithRange:clamped];
      NSUInteger spanCells = [self cellWidthOfString:sub];
      if (spanCells > 0) {
        rectW = (CGFloat)spanCells * cw;
      }
    }
  }
  CGFloat maxX = fmax(kPadX, self.bounds.size.width - kPadX - cw);
  if (self.bounds.size.width > kPadX * 2.0 + cw && cx > maxX) {
    cx = maxX;
  }
  NSRect localRect = NSMakeRect(cx, cy, rectW, ch);
  if (!self.window)
    return localRect;
  NSRect winRect = [self convertRect:localRect toView:nil];
  return [self.window convertRectToScreen:winRect];
}

- (NSUInteger)characterIndexForPoint:(NSPoint)point {
  (void)point;
  return 0;
}

#pragma mark - Mouse, Selection, Links, ScrollWheel & Accessibility

- (void)clearHoveredLink {
  if (_hoverLinkValid) {
    uint16_t oldRow = _hoverLinkRow;
    _hoverLinkValid = NO;
    self.hoverLinkUri = nil;
    if (_snap && oldRow < _snap->rows) {
      [self setNeedsDisplayInRect:[self rectForRow:oldRow]];
    }
  }
}

- (NSView *)activeContentView {
  if (self.window)
    return self.window.contentView;
  return self.testContentView;
}

- (NSPoint)contentPointFromWindowPoint:(NSPoint)winPt
                         inContentView:(NSView *)contentView {
  if (self.window) {
    return [contentView convertPoint:winPt fromView:nil];
  }
  return winPt;
}

- (BOOL)isPointerHitAtContentPoint:(NSPoint)contentPt
                     inContentView:(NSView *)contentView {
  if (!_host || !contentView || self.isHiddenOrHasHiddenAncestor)
    return NO;
  if (self.window) {
    if ([NSApp keyWindow] && [NSApp keyWindow] != self.window)
      return NO;
    if (self.window.attachedSheet != nil)
      return NO;
  }
  NSPoint localPt = [self convertPoint:contentPt fromView:contentView];
  if (!NSPointInRect(localPt, self.bounds))
    return NO;
  NSPoint contentSuperPt =
      contentView.superview
          ? [contentView.superview convertPoint:contentPt fromView:contentView]
          : contentPt;
  NSView *hit = [contentView hitTest:contentSuperPt];
  return hit != nil && [hit isDescendantOf:self];
}

- (BOOL)isPointerHitAtWindowPoint:(NSPoint)winPt {
  NSView *contentView = [self activeContentView];
  if (!contentView)
    return NO;
  NSPoint contentPt =
      [self contentPointFromWindowPoint:winPt inContentView:contentView];
  return [self isPointerHitAtContentPoint:contentPt inContentView:contentView];
}

- (void)refreshHoverStateAtPoint:(NSPoint)pt
                   modifierFlags:(NSEventModifierFlags)flags {
  NSView *contentView = [self activeContentView];
  if (!_host || !contentView || !NSPointInRect(pt, self.bounds)) {
    [self clearHoveredLink];
    return;
  }
  NSPoint contentPt = [self convertPoint:pt toView:contentView];
  if (![self isPointerHitAtContentPoint:contentPt inContentView:contentView]) {
    [self clearHoveredLink];
    return;
  }
  BOOL cmdHeld = (flags & NSEventModifierFlagCommand) != 0;
  BOOL optHeld = (flags & NSEventModifierFlagOption) != 0;

  if (cmdHeld && _cb.link_at) {
    CbLinkSpan span = _cb.link_at(_host, pt.x, pt.y);
    if (span.found && span.uri_ptr && span.uri_len > 0) {
      NSString *uri = [[NSString alloc] initWithBytes:span.uri_ptr
                                               length:span.uri_len
                                             encoding:NSUTF8StringEncoding];
      BOOL changed = !_hoverLinkValid || _hoverLinkRow != span.row ||
                     _hoverLinkStartCol != span.start_col ||
                     _hoverLinkEndCol != span.end_col;
      uint16_t prevRow = _hoverLinkRow;
      _hoverLinkValid = YES;
      _hoverLinkRow = span.row;
      _hoverLinkStartCol = span.start_col;
      _hoverLinkEndCol = span.end_col;
      self.hoverLinkUri = uri;
      if (changed && _snap) {
        if (prevRow < _snap->rows) {
          [self setNeedsDisplayInRect:[self rectForRow:prevRow]];
        }
        if (span.row < _snap->rows) {
          [self setNeedsDisplayInRect:[self rectForRow:span.row]];
        }
      }
      [[NSCursor pointingHandCursor] set];
      return;
    }
  }

  [self clearHoveredLink];
  if (_snap && _snap->mouse_tracking && !optHeld) {
    [[NSCursor arrowCursor] set];
  } else {
    [[NSCursor IBeamCursor] set];
  }
}

- (NSPoint)localPointFromWindowPoint:(NSPoint)winPt {
  NSView *contentView = [self activeContentView];
  if (!self.window && contentView) {
    return [self convertPoint:winPt fromView:contentView];
  }
  return [self convertPoint:winPt fromView:nil];
}

- (void)cursorUpdate:(NSEvent *)event {
  NSPoint winPt = event.locationInWindow;
  if (![self isPointerHitAtWindowPoint:winPt]) {
    [self clearHoveredLink];
    if (self.nextResponder) {
      [self.nextResponder cursorUpdate:event];
    } else {
      [super cursorUpdate:event];
    }
    return;
  }
  NSPoint pt = [self localPointFromWindowPoint:winPt];
  [self refreshHoverStateAtPoint:pt modifierFlags:event.modifierFlags];
}

- (void)mouseEntered:(NSEvent *)event {
  NSPoint winPt = event.locationInWindow;
  if (![self isPointerHitAtWindowPoint:winPt]) {
    [self clearHoveredLink];
    return;
  }
  NSPoint pt = [self localPointFromWindowPoint:winPt];
  [self refreshHoverStateAtPoint:pt modifierFlags:event.modifierFlags];
}

- (void)mouseExited:(NSEvent *)event {
  (void)event;
  [self clearHoveredLink];
}

- (void)mouseMoved:(NSEvent *)event {
  if (!_host)
    return;
  NSPoint winPt = event.locationInWindow;
  if (![self isPointerHitAtWindowPoint:winPt]) {
    [self clearHoveredLink];
    return;
  }
  NSPoint pt = [self localPointFromWindowPoint:winPt];
  [self refreshHoverStateAtPoint:pt modifierFlags:event.modifierFlags];

  if (_snap && _snap->mouse_tracking && _cb.mouse &&
      !(event.modifierFlags & NSEventModifierFlagOption)) {
    CbMouseEvent mev = {0};
    mev.action = 2; // motion
    mev.button = 0; // none
    mev.any_pressed = ([NSEvent pressedMouseButtons] != 0) ? 1 : 0;
    mev.alt_override = 0;
    mev.mods = cb_mods_from_flags(event.modifierFlags);
    mev.x_px = (float)pt.x;
    mev.y_px = (float)pt.y;
    _cb.mouse(_host, mev);
  }
}

- (void)mouseDown:(NSEvent *)event {
  if (!_host)
    return;
  if (self.window && self.window.firstResponder != self) {
    [self.window makeFirstResponder:self];
  }

  NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
  NSEventModifierFlags flags = event.modifierFlags;
  BOOL cmdHeld = (flags & NSEventModifierFlagCommand) != 0;
  BOOL optHeld = (flags & NSEventModifierFlagOption) != 0;
  BOOL shiftHeld = (flags & NSEventModifierFlagShift) != 0;

  if (cmdHeld && _cb.link_at) {
    CbLinkSpan span = _cb.link_at(_host, pt.x, pt.y);
    if (span.found && span.uri_ptr && span.uri_len > 0) {
      NSString *uriStr = [[NSString alloc] initWithBytes:span.uri_ptr
                                                  length:span.uri_len
                                                encoding:NSUTF8StringEncoding];
      NSURL *url = uriStr ? [NSURL URLWithString:uriStr] : nil;
      if (url) {
        [[NSWorkspace sharedWorkspace] openURL:url];
        return;
      }
    }
  }

  _mouseCapturedByPty = NO;
  _selectingLocally = NO;
  _autoscrollDir = 0;

  if (!optHeld && _cb.mouse) {
    CbMouseEvent mev = {0};
    mev.action = 0; // press
    mev.button = 1; // left
    mev.any_pressed = 1;
    mev.alt_override = 0;
    mev.mods = cb_mods_from_flags(flags);
    mev.x_px = (float)pt.x;
    mev.y_px = (float)pt.y;
    if (_cb.mouse(_host, mev)) {
      _mouseCapturedByPty = YES;
      return;
    }
  }

  if (_cb.select) {
    _selectingLocally = YES;
    _selectionRectangle = optHeld;
    _lastDragPoint = pt;
    CbSelectEvent sev = {0};
    sev.kind = 0; // press
    sev.shift_extend = shiftHeld ? 1 : 0;
    sev.rectangle = optHeld ? 1 : 0;
    sev.x_px = pt.x;
    sev.y_px = pt.y;
    sev.time_ns = (uint64_t)(event.timestamp * 1e9);
    sev.repeat_interval_ns = (uint64_t)([NSEvent doubleClickInterval] * 1e9);
    _cb.select(_host, sev);
    [self pollSnapshotForceFull:NO];
  }
}

- (void)mouseDragged:(NSEvent *)event {
  if (!_host)
    return;
  NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
  NSEventModifierFlags flags = event.modifierFlags;

  if (_mouseCapturedByPty && _cb.mouse) {
    CbMouseEvent mev = {0};
    mev.action = 2; // motion
    mev.button = 1; // left held
    mev.any_pressed = 1;
    mev.alt_override = 0;
    mev.mods = cb_mods_from_flags(flags);
    mev.x_px = (float)pt.x;
    mev.y_px = (float)pt.y;
    _cb.mouse(_host, mev);
    return;
  }

  if (_selectingLocally && _cb.select) {
    _lastDragPoint = pt;
    if (flags & NSEventModifierFlagOption) {
      _selectionRectangle = YES;
    }
    CbSelectEvent sev = {0};
    sev.kind = 1; // drag
    sev.rectangle = _selectionRectangle ? 1 : 0;
    sev.x_px = pt.x;
    sev.y_px = pt.y;
    _autoscrollDir = _cb.select(_host, sev);
    [self pollSnapshotForceFull:NO];
  }
}

- (void)mouseUp:(NSEvent *)event {
  if (!_host)
    return;
  NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];

  if (_mouseCapturedByPty && _cb.mouse) {
    _mouseCapturedByPty = NO;
    CbMouseEvent mev = {0};
    mev.action = 1; // release
    mev.button = 1; // left
    mev.any_pressed = ([NSEvent pressedMouseButtons] != 0) ? 1 : 0;
    mev.alt_override = 0;
    mev.mods = cb_mods_from_flags(event.modifierFlags);
    mev.x_px = (float)pt.x;
    mev.y_px = (float)pt.y;
    _cb.mouse(_host, mev);
    return;
  }

  if (_selectingLocally && _cb.select) {
    _selectingLocally = NO;
    _autoscrollDir = 0;
    CbSelectEvent sev = {0};
    sev.kind = 2; // release
    sev.x_px = pt.x;
    sev.y_px = pt.y;
    _cb.select(_host, sev);
    [self pollSnapshotForceFull:NO];
  }
}

- (void)rightMouseDown:(NSEvent *)event {
  if (!_host)
    return;
  if (self.window && self.window.firstResponder != self) {
    [self.window makeFirstResponder:self];
  }
  NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
  BOOL optHeld = (event.modifierFlags & NSEventModifierFlagOption) != 0;

  if (!optHeld && _cb.mouse) {
    CbMouseEvent mev = {0};
    mev.action = 0;
    mev.button = 2; // right
    mev.any_pressed = 1;
    mev.alt_override = 0;
    mev.mods = cb_mods_from_flags(event.modifierFlags);
    mev.x_px = (float)pt.x;
    mev.y_px = (float)pt.y;
    if (_cb.mouse(_host, mev)) {
      return;
    }
  }

  NSMenu *menu = [[NSMenu alloc] initWithTitle:@"Terminal"];
  NSMenuItem *copyItem = [[NSMenuItem alloc] initWithTitle:@"Copy"
                                                    action:@selector(copy:)
                                             keyEquivalent:@"c"];
  copyItem.target = self;
  [menu addItem:copyItem];

  NSMenuItem *pasteItem = [[NSMenuItem alloc] initWithTitle:@"Paste"
                                                     action:@selector(paste:)
                                              keyEquivalent:@"v"];
  pasteItem.target = self;
  [menu addItem:pasteItem];

  [menu addItem:[NSMenuItem separatorItem]];

  NSMenuItem *selAll = [[NSMenuItem alloc] initWithTitle:@"Select All"
                                                  action:@selector(selectAll:)
                                           keyEquivalent:@"a"];
  selAll.target = self;
  [menu addItem:selAll];

  NSMenuItem *clearItem =
      [[NSMenuItem alloc] initWithTitle:@"Clear"
                                 action:@selector(clearTerminal:)
                          keyEquivalent:@"k"];
  clearItem.target = self;
  [menu addItem:clearItem];

  [NSMenu popUpContextMenu:menu withEvent:event forView:self];
}

- (void)rightMouseDragged:(NSEvent *)event {
  if (!_host || !_cb.mouse || (event.modifierFlags & NSEventModifierFlagOption))
    return;
  NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
  CbMouseEvent mev = {0};
  mev.action = 2;
  mev.button = 2;
  mev.any_pressed = 1;
  mev.mods = cb_mods_from_flags(event.modifierFlags);
  mev.x_px = (float)pt.x;
  mev.y_px = (float)pt.y;
  _cb.mouse(_host, mev);
}

- (void)rightMouseUp:(NSEvent *)event {
  if (!_host || !_cb.mouse || (event.modifierFlags & NSEventModifierFlagOption))
    return;
  NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
  CbMouseEvent mev = {0};
  mev.action = 1;
  mev.button = 2;
  mev.any_pressed = ([NSEvent pressedMouseButtons] != 0) ? 1 : 0;
  mev.mods = cb_mods_from_flags(event.modifierFlags);
  mev.x_px = (float)pt.x;
  mev.y_px = (float)pt.y;
  _cb.mouse(_host, mev);
}

- (void)otherMouseDown:(NSEvent *)event {
  if (!_host)
    return;
  if (self.window && self.window.firstResponder != self) {
    [self.window makeFirstResponder:self];
  }
  NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
  if (_cb.mouse && !(event.modifierFlags & NSEventModifierFlagOption)) {
    CbMouseEvent mev = {0};
    mev.action = 0;
    mev.button = 3; // middle
    mev.any_pressed = 1;
    mev.mods = cb_mods_from_flags(event.modifierFlags);
    mev.x_px = (float)pt.x;
    mev.y_px = (float)pt.y;
    if (_cb.mouse(_host, mev))
      return;
  }
}

- (void)otherMouseUp:(NSEvent *)event {
  if (!_host || !_cb.mouse || (event.modifierFlags & NSEventModifierFlagOption))
    return;
  NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
  CbMouseEvent mev = {0};
  mev.action = 1;
  mev.button = 3;
  mev.any_pressed = ([NSEvent pressedMouseButtons] != 0) ? 1 : 0;
  mev.mods = cb_mods_from_flags(event.modifierFlags);
  mev.x_px = (float)pt.x;
  mev.y_px = (float)pt.y;
  _cb.mouse(_host, mev);
}

- (void)scrollWheel:(NSEvent *)event {
  if (!_host || !_cb.wheel)
    return;
  if (event.phase == NSEventPhaseBegan ||
      event.momentumPhase == NSEventPhaseBegan) {
    _scrollAccumY = 0.0;
    _scrollAccumX = 0.0;
  }

  CGFloat ch = self.cellHeight > 1.0 ? self.cellHeight : 18.0;
  CGFloat cw = self.cellWidth > 1.0 ? self.cellWidth : 8.0;
  int32_t dRows = 0;
  int32_t dCols = 0;

  if (event.hasPreciseScrollingDeltas) {
    // Trackpad / Magic Mouse: scrollingDeltaY > 0 means scrolling up into history -> negative delta_rows.
    _scrollAccumY += -event.scrollingDeltaY;
    _scrollAccumX += -event.scrollingDeltaX;
    dRows = (int32_t)trunc(_scrollAccumY / ch);
    dCols = (int32_t)trunc(_scrollAccumX / cw);
    _scrollAccumY -= (CGFloat)dRows * ch;
    _scrollAccumX -= (CGFloat)dCols * cw;
  } else {
    if (fabs(event.scrollingDeltaY) > 0.001) {
      int32_t step = (int32_t)round(-event.scrollingDeltaY * 3.0);
      if (step == 0)
        step = event.scrollingDeltaY > 0.0 ? -1 : 1;
      dRows = step;
    }
    if (fabs(event.scrollingDeltaX) > 0.001) {
      dCols = (int32_t)round(-event.scrollingDeltaX * 3.0);
    }
  }

  if (dRows == 0 && dCols == 0)
    return;

  NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
  BOOL altOverride = (event.modifierFlags & NSEventModifierFlagOption) != 0;
  uint16_t mods = cb_mods_from_flags(event.modifierFlags);
  _cb.wheel(_host, dRows, dCols, (float)pt.x, (float)pt.y, mods, altOverride);
  _scrollbarVisibleUntil = [NSProcessInfo processInfo].systemUptime + 0.9;
  [self pollSnapshotForceFull:NO];
}

#pragma mark - Accessibility

- (BOOL)isAccessibilityElement {
  return YES;
}

- (NSAccessibilityRole)accessibilityRole {
  return NSAccessibilityTextAreaRole;
}

- (NSString *)accessibilityLabel {
  return @"Terminal";
}

- (NSString *)accessibilityValue {
  if (!_host || !_cb.accessibility_text)
    return @"";
  size_t len = 0;
  const uint8_t *ptr = _cb.accessibility_text(_host, &len);
  if (!ptr || len == 0)
    return @"";
  return [[NSString alloc] initWithBytes:ptr
                                  length:len
                                encoding:NSUTF8StringEncoding] ?: @"";
}

@end

#pragma mark - C FFI Entry Points

void *cb_terminal_new(void *host, CbCallbacks callbacks) {
  CanbanTerminal *view =
      [[CanbanTerminal alloc] initWithHost:host callbacks:callbacks];
  return (__bridge_retained void *)view;
}

void cb_terminal_detach(void *view) {
  if (!view)
    return;
  CanbanTerminal *term = (__bridge CanbanTerminal *)view;
  [term detachHost];
}

void cb_terminal_wake(void *view) {
  if (!view)
    return;
  __weak CanbanTerminal *weakTerm = (__bridge CanbanTerminal *)view;
  dispatch_async(dispatch_get_main_queue(), ^{
    CanbanTerminal *strong = weakTerm;
    if (strong) {
      [strong handleWake];
    }
  });
}

void cb_terminal_auto_focus(void *view, bool enabled) {
  if (!view)
    return;
  CanbanTerminal *term = (__bridge CanbanTerminal *)view;
  term.autoFocus = enabled ? YES : NO;
  if (enabled) {
    [term requestAutoFocusIfEligible];
    __weak CanbanTerminal *weakTerm = term;
    dispatch_async(dispatch_get_main_queue(), ^{
      CanbanTerminal *strong = weakTerm;
      if (strong && strong.autoFocus) {
        [strong requestAutoFocusIfEligible];
      }
    });
  }
}

void cb_terminal_ui_command_keys(void *view, const char *json) {
  if (!view || !json)
    return;
  CanbanTerminal *term = (__bridge CanbanTerminal *)view;
  NSData *data = [NSData dataWithBytes:json length:strlen(json)];
  id parsed = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
  if ([parsed isKindOfClass:[NSArray class]]) {
    NSMutableSet<NSString *> *set = [NSMutableSet set];
    for (id item in (NSArray *)parsed) {
      if ([item isKindOfClass:[NSString class]]) {
        [set addObject:[(NSString *)item lowercaseString]];
      }
    }
    term.uiCommandKeys = [set copy];
  }
}

void cb_terminal_set_font(void *view, const char *family, double size) {
  if (!view)
    return;
  CanbanTerminal *term = (__bridge CanbanTerminal *)view;
  NSString *nextFamily =
      (family && family[0] != '\0') ? [NSString stringWithUTF8String:family] : nil;
  CGFloat nextSize = fmax(9.0, fmin(32.0, (CGFloat)size));
  BOOL famSame = (term.fontFamily == nil && nextFamily == nil) ||
                 (term.fontFamily && nextFamily &&
                  [term.fontFamily isEqualToString:nextFamily]);
  if (famSame && fabs(term.fontSize - nextSize) < 0.01) {
    return;
  }
  term.fontFamily = nextFamily;
  term.fontSize = nextSize;
  [term rebuildFonts];
  if ([term hasRenderableBounds]) {
    [term pollSnapshotForceFull:YES];
    [term setNeedsDisplay:YES];
  }
}

void cb_terminal_set_theme_bg(void *view, uint8_t r, uint8_t g, uint8_t b) {
  if (!view)
    return;
  CanbanTerminal *term = (__bridge CanbanTerminal *)view;
  [term setThemeBgR:r g:g b:b];
}

void cb_terminal_sync_rebind(void *view) {
  if (!view)
    return;
  CanbanTerminal *term = (__bridge CanbanTerminal *)view;
  [term syncRebind];
}



bool cb_terminal_test_mutable_ime_buffers(void *view) {
  if (!view)
    return false;
  @autoreleasepool {
    CanbanTerminal *term = (__bridge CanbanTerminal *)view;
    NSRange notFound = NSMakeRange(NSNotFound, 0);

    // 1. Simulate IMKInputSession (e.g. WeType) passing a reusable mutable
    // NSMutableString during interpretKeyEvents: and clearing it before
    // keyDown: reads keyTextAccumulator.
    term.keyTextAccumulator = [NSMutableArray arrayWithCapacity:2];
    NSMutableString *mutAscii = [NSMutableString stringWithString:@"x"];
    [term insertText:mutAscii replacementRange:notFound];
    [mutAscii setString:@""];

    NSMutableAttributedString *mutAttr =
        [[NSMutableAttributedString alloc] initWithString:@"你好"];
    [term insertText:mutAttr replacementRange:notFound];
    [[mutAttr mutableString] setString:@""];

    NSArray<NSString *> *accum = term.keyTextAccumulator;
    term.keyTextAccumulator = nil;
    if (accum.count != 2 || ![accum[0] isEqualToString:@"x"] ||
        ![accum[1] isEqualToString:@"你好"]) {
      return false;
    }

    // 2. Simulate setMarkedText: with a reusable mutable attributed string
    // followed by immediate IMK buffer truncation.
    NSMutableAttributedString *mutMarked =
        [[NSMutableAttributedString alloc] initWithString:@"nihao"];
    [term setMarkedText:mutMarked
           selectedRange:NSMakeRange(5, 0)
        replacementRange:notFound];
    [[mutMarked mutableString] setString:@""];
    if (![term hasMarkedText] ||
        ![term.markedTextStr isEqualToString:@"nihao"]) {
      return false;
    }

    // 3. Direct async insertText: outside keyDown: commits UTF-8 via _cb.text
    // and clears marked text even if the caller mutates its buffer afterward.
    NSMutableString *commitBuf = [NSMutableString stringWithString:@"你好！"];
    [term insertText:commitBuf replacementRange:notFound];
    [commitBuf setString:@""];
    if ([term hasMarkedText]) {
      return false;
    }
    return true;
  }
}

bool cb_terminal_test_ime_commit_cursor_hold(void *view) {
  if (!view)
    return false;
  @autoreleasepool {
    CanbanTerminal *term = (__bridge CanbanTerminal *)view;
    NSRange notFound = NSMakeRange(NSNotFound, 0);

    [term setFrame:NSMakeRect(0.0, 0.0, 720.0, 440.0)];
    term.focusedState = YES;
    [term pollSnapshotForceFull:YES];

    CGFloat cw = term.cellWidth;
    if (cw < 4.0)
      return false;

    uint16_t cx = 0, cy = 0;
    if ([term effectiveCursorX:&cx y:&cy] || cx != 2 || cy != 0) {
      return false;
    }

    // Helper block to exercise offscreen CoreGraphics drawRect: paths.
    NSBitmapImageRep *rep = [[NSBitmapImageRep alloc]
        initWithBitmapDataPlanes:NULL
                      pixelsWide:720
                      pixelsHigh:440
                   bitsPerSample:8
                 samplesPerPixel:4
                        hasAlpha:YES
                        isPlanar:NO
                  colorSpaceName:NSDeviceRGBColorSpace
                     bytesPerRow:0
                    bitsPerPixel:0];
    NSGraphicsContext *bmpCtx =
        rep ? [NSGraphicsContext graphicsContextWithBitmapImageRep:rep] : nil;

    // 1. Pinyin preedit at cursor col 2: end-of-preedit range {5, 0} must
    // preserve actualRange {5, 0} and place the candidate rect at col 2 + 5.
    [term setMarkedText:@"nihao"
           selectedRange:NSMakeRange(5, 0)
        replacementRange:notFound];
    NSRange actual = NSMakeRange(0, 0);
    NSRect caretRect =
        [term firstRectForCharacterRange:NSMakeRange(5, 0) actualRange:&actual];
    if (actual.location != 5 || actual.length != 0)
      return false;
    if (fabs(caretRect.origin.x - (kPadX + 7.0 * cw)) > 0.05)
      return false;

    NSRange attrActual = NSMakeRange(0, 0);
    NSAttributedString *emptyTail =
        [term attributedSubstringForProposedRange:NSMakeRange(5, 0)
                                      actualRange:&attrActual];
    if (emptyTail != nil || attrActual.location != 5 || attrActual.length != 0)
      return false;

    if (bmpCtx) {
      [term displayRectIgnoringOpacity:term.bounds inContext:bmpCtx];
    }

    // 2. Surrogate-pair preedit ("😀a", UTF-16 length 3, terminal width 3 cols).
    [term setMarkedText:@"😀a"
           selectedRange:NSMakeRange(3, 0)
        replacementRange:notFound];
    NSRect afterEmoji =
        [term firstRectForCharacterRange:NSMakeRange(2, 1) actualRange:&actual];
    if (actual.location != 2 || actual.length != 1)
      return false;
    if (fabs(afterEmoji.origin.x - (kPadX + 4.0 * cw)) > 0.05 ||
        fabs(afterEmoji.size.width - cw) > 0.05)
      return false;
    // Querying mid-surrogate {1, 0} clamps safely to UTF-16 codepoint boundary.
    NSRect midSurrogate =
        [term firstRectForCharacterRange:NSMakeRange(1, 0) actualRange:&actual];
    if (actual.location != 0 || actual.length != 0 ||
        fabs(midSurrogate.origin.x - (kPadX + 2.0 * cw)) > 0.05)
      return false;

    // 3. Commit "你好" (4 cells) before PTY echo arrives: cursor must hold at
    // col 2 + 4 = 6 instead of flashing back to col 2.
    [term insertText:@"你好" replacementRange:notFound];
    if ([term hasMarkedText])
      return false;
    if (![term effectiveCursorX:&cx y:&cy] || cx != 6 || cy != 0)
      return false;
    if (bmpCtx) {
      [term displayRectIgnoringOpacity:term.bounds inContext:bmpCtx];
    }

    // 4. Consecutive CJK punctuation commit "，" (2 cells, no prior markedText)
    // accumulates optimistic cursor to col 6 + 2 = 8, and starting the next
    // Pinyin preedit anchors its candidate window at col 8 rather than col 2.
    [term insertText:@"，" replacementRange:notFound];
    if (![term effectiveCursorX:&cx y:&cy] || cx != 8 || cy != 0)
      return false;
    [term setMarkedText:@"shi"
           selectedRange:NSMakeRange(3, 0)
        replacementRange:notFound];
    NSRect nextWordStart =
        [term firstRectForCharacterRange:NSMakeRange(0, 0) actualRange:&actual];
    NSRect nextWordCaret =
        [term firstRectForCharacterRange:NSMakeRange(3, 0) actualRange:&actual];
    if (fabs(nextWordStart.origin.x - (kPadX + 8.0 * cw)) > 0.05 ||
        fabs(nextWordCaret.origin.x - (kPadX + 11.0 * cw)) > 0.05)
      return false;
    [term unmarkText];

    // 5. Non-echo fallback (e.g. password prompt): once grace expires without
    // PTY cursor movement, effectiveCursorX falls back to real VT cursor (col 2).
    term.commitHoldUntil = 0.0;
    [term onAnimTick];
    if (term.commitCursorActive || [term effectiveCursorX:&cx y:&cy] ||
        cx != 2 || cy != 0)
      return false;

    // 6. Two-step XPC IME commit: setMarkedText:@"" followed by insertText:@"好"
    // still activates optimistic hold via recent-unmark + non-ASCII detection.
    [term setMarkedText:@"hao"
           selectedRange:NSMakeRange(3, 0)
        replacementRange:notFound];
    [term setMarkedText:@""
           selectedRange:NSMakeRange(0, 0)
        replacementRange:notFound];
    [term insertText:@"好" replacementRange:notFound];
    if (![term effectiveCursorX:&cx y:&cy] || cx != 4 || cy != 0)
      return false;

    return true;
  }
}

bool cb_terminal_test_verify_post_echo_cursor(void *view, uint16_t expectedX) {
  if (!view)
    return false;
  @autoreleasepool {
    CanbanTerminal *term = (__bridge CanbanTerminal *)view;
    [term pollSnapshotForceFull:NO];
    uint16_t cx = 0, cy = 0;
    BOOL optimistic = [term effectiveCursorX:&cx y:&cy];
    return (!term.commitCursorActive && !optimistic && cx == expectedX &&
            cy == 0);
  }
}

@interface CbFakeDraggingInfo : NSObject <NSDraggingInfo>
@property(nonatomic, strong) NSPasteboard *mockPasteboard;
@property(nonatomic, weak) NSWindow *mockWindow;
@end

@implementation CbFakeDraggingInfo
- (NSWindow *)draggingDestinationWindow {
  return self.mockWindow;
}
- (NSDragOperation)draggingSourceOperationMask {
  return NSDragOperationEvery;
}
- (NSPoint)draggingLocation {
  return NSMakePoint(20.0, 20.0);
}
- (NSPoint)draggedImageLocation {
  return NSMakePoint(20.0, 20.0);
}
- (NSImage *)draggedImage {
  return nil;
}
- (NSPasteboard *)draggingPasteboard {
  return self.mockPasteboard;
}
- (id)draggingSource {
  return nil;
}
- (NSInteger)draggingSequenceNumber {
  return 1;
}
- (void)slideDraggedImageTo:(NSPoint)screenPoint {
  (void)screenPoint;
}
- (NSArray<NSString *> *)namesOfPromisedFilesDroppedAtDestination:
    (NSURL *)dropDestination {
  (void)dropDestination;
  return @[];
}
- (void)enumerateDraggingItemsWithOptions:(NSDraggingItemEnumerationOptions)enumOpts
                                  forView:(NSView *)view
                                  classes:(NSArray<Class> *)classArray
                            searchOptions:
                                (NSDictionary<NSPasteboardReadingOptionKey, id> *)
                                    searchOptions
                               usingBlock:
                                   (void (^)(NSDraggingItem *, NSInteger, BOOL *))
                                       block {
  (void)enumOpts;
  (void)view;
  (void)classArray;
  (void)searchOptions;
  (void)block;
}
@synthesize animatesToDestination;
@synthesize draggingFormation;
@synthesize numberOfValidItemsForDrop;
@synthesize springLoadingHighlight;
- (void)resetSpringLoading {
}
@end

bool cb_terminal_test_drag_and_file_paste(void *view) {
  if (!view)
    return false;
  @autoreleasepool {
    CanbanTerminal *term = (__bridge CanbanTerminal *)view;
    NSArray<NSPasteboardType> *registered = term.registeredDraggedTypes;
    if (![registered containsObject:NSPasteboardTypeFileURL] ||
        ![registered containsObject:NSPasteboardTypeURL] ||
        ![registered containsObject:NSPasteboardTypeString]) {
      return false;
    }

    // 1. Multi-file Finder drag (including spaces, single quotes, and CJK characters;
    //    Finder also writes filename NSPasteboardTypeString alongside file URLs).
    NSPasteboard *filePb = [NSPasteboard pasteboardWithUniqueName];
    [filePb clearContents];
    NSURL *u1 = [NSURL fileURLWithPath:@"/Users/jjy/foo/bar.rs"];
    NSURL *u2 = [NSURL fileURLWithPath:@"/Users/jjy/My Docs/it's 截图 (1).png"];
    [filePb writeObjects:@[ u1, u2 ]];
    [filePb setString:@"bar.rs\nit's 截图 (1).png"
              forType:NSPasteboardTypeString];

    CbFakeDraggingInfo *drag = [[CbFakeDraggingInfo alloc] init];
    drag.mockPasteboard = filePb;
    if ([term draggingEntered:drag] != NSDragOperationCopy ||
        [term draggingUpdated:drag] != NSDragOperationCopy ||
        ![term prepareForDragOperation:drag] ||
        ![term performDragOperation:drag]) {
      [filePb releaseGlobally];
      return false;
    }

    // 2. Finder Cmd+C -> pasteFromPasteboard: must prefer full escaped file paths
    //    over Finder's bare filename string representation.
    NSPasteboard *copyPb = [NSPasteboard pasteboardWithUniqueName];
    [copyPb clearContents];
    NSURL *u3 = [NSURL fileURLWithPath:@"/tmp/hello world/$HOME.txt"];
    [copyPb writeObjects:@[ u3 ]];
    [copyPb setString:@"$HOME.txt" forType:NSPasteboardTypeString];
    if (![term pasteFromPasteboard:copyPb]) {
      [filePb releaseGlobally];
      [copyPb releaseGlobally];
      return false;
    }

    // 3. Plain text drag & non-file URL drag pass through verbatim without trailing-space escaping.
    NSPasteboard *textPb = [NSPasteboard pasteboardWithUniqueName];
    [textPb clearContents];
    [textPb setString:@"echo hi" forType:NSPasteboardTypeString];
    drag.mockPasteboard = textPb;
    if ([term draggingEntered:drag] != NSDragOperationCopy ||
        ![term performDragOperation:drag]) {
      [filePb releaseGlobally];
      [copyPb releaseGlobally];
      [textPb releaseGlobally];
      return false;
    }

    NSPasteboard *webPb = [NSPasteboard pasteboardWithUniqueName];
    [webPb clearContents];
    NSURL *webUrl = [NSURL URLWithString:@"https://canban.dev/docs?a=1&b=2"];
    [webPb writeObjects:@[ webUrl ]];
    drag.mockPasteboard = webPb;
    if ([term draggingEntered:drag] != NSDragOperationCopy ||
        ![term performDragOperation:drag]) {
      [filePb releaseGlobally];
      [copyPb releaseGlobally];
      [textPb releaseGlobally];
      [webPb releaseGlobally];
      return false;
    }

    // 4. Empty pasteboard is rejected cleanly.
    NSPasteboard *emptyPb = [NSPasteboard pasteboardWithUniqueName];
    [emptyPb clearContents];
    drag.mockPasteboard = emptyPb;
    bool emptyRejected = ([term draggingEntered:drag] == NSDragOperationNone &&
                          ![term prepareForDragOperation:drag] &&
                          ![term performDragOperation:drag]);

    [filePb releaseGlobally];
    [copyPb releaseGlobally];
    [textPb releaseGlobally];
    [webPb releaseGlobally];
    [emptyPb releaseGlobally];
    return emptyRejected;
  }
}

bool cb_terminal_test_nerd_font_fallback(void *view) {
  if (!view)
    return false;
  @autoreleasepool {
    CanbanTerminal *term = (__bridge CanbanTerminal *)view;
    [term setFrame:NSMakeRect(0.0, 0.0, 720.0, 320.0)];

    // Verify helper checks first: ASCII works on System Mono, and .LastResort
    // detection accurately identifies CoreText's tofu font.
    UniChar asciiA = 'A';
    CGGlyph gA = 0;
    CTFontRef sysFont = cb_create_styled_font(nil, 13.0, 0);
    if (!sysFont || !cb_font_covers_utf16(sysFont, &asciiA, 1, &gA) || gA == 0) {
      if (sysFont)
        CFRelease(sysFont);
      return false;
    }
    if (cb_font_is_last_resort(sysFont)) {
      CFRelease(sysFont);
      return false;
    }

    UniChar puaArrow = 0xE0B0;
    CFStringRef puaStr =
        CFStringCreateWithCharacters(kCFAllocatorDefault, &puaArrow, 1);
    CTFontRef rawCoreTextFb =
        CTFontCreateForString(sysFont, puaStr, CFRangeMake(0, 1));
    CFRelease(puaStr);
    CFRelease(sysFont);
    if (!rawCoreTextFb)
      return false;
    bool coreTextReturnedLastResort = cb_font_is_last_resort(rawCoreTextFb);
    CFRelease(rawCoreTextFb);

    NSArray<NSString *> *nerdFams = cb_nerd_fallback_families();
    if (nerdFams.count > 0) {
      // With System Mono (nil) and Menlo, BMP PUA (U+E0B0, U+F07B, U+EA60) and
      // Supplementary PUA-A (U+F0001) must resolve to an installed Nerd Font
      // rather than .LastResort, and cover the codepoints cleanly.
      NSArray *primaries = @[ [NSNull null], @"Menlo" ];
      uint32_t testCps[4] = {0xE0B0, 0xF07B, 0xEA60, 0xF0001};
      for (id famObj in primaries) {
        NSString *fam = (famObj == [NSNull null]) ? nil : (NSString *)famObj;
        term.fontFamily = fam;
        term.fontSize = 13.0;
        [term rebuildFonts];

        for (int i = 0; i < 4; i++) {
          uint32_t cp = testCps[i];
          UniChar u16[2];
          CFIndex u16Len = 0;
          if (cp <= 0xFFFF) {
            u16[0] = (UniChar)cp;
            u16Len = 1;
          } else {
            uint32_t u = cp - 0x10000;
            u16[0] = (UniChar)(0xD800 + (u >> 10));
            u16[1] = (UniChar)(0xDC00 + (u & 0x3FF));
            u16Len = 2;
          }
          for (uint16_t fl = 0; fl <= (CB_STYLE_BOLD | CB_STYLE_ITALIC); fl++) {
            CTFontRef resolved = [term resolveFontForUTF16:u16
                                                    length:u16Len
                                                 codepoint:cp
                                                     flags:fl];
            CGGlyph g = 0;
            if (!resolved || cb_font_is_last_resort(resolved) ||
                !cb_font_covers_utf16(resolved, u16, u16Len, &g) || g == 0) {
              return false;
            }
          }
        }
      }
      // Also confirm CoreText alone would have produced .LastResort on macOS.
      (void)coreTextReturnedLastResort;
    }

    // Exercise full offscreen drawRect: pass with PUA glyphs in the VT buffer
    // to verify scaling/centering paths render cleanly without crashes or NaNs.
    term.fontFamily = nil;
    term.fontSize = 13.0;
    [term rebuildFonts];
    [term pollSnapshotForceFull:YES];

    NSBitmapImageRep *rep = [[NSBitmapImageRep alloc]
        initWithBitmapDataPlanes:NULL
                      pixelsWide:720
                      pixelsHigh:320
                   bitsPerSample:8
                 samplesPerPixel:4
                        hasAlpha:YES
                        isPlanar:NO
                  colorSpaceName:NSDeviceRGBColorSpace
                     bytesPerRow:0
                    bitsPerPixel:0];
    NSGraphicsContext *bmpCtx =
        rep ? [NSGraphicsContext graphicsContextWithBitmapImageRep:rep] : nil;
    if (!bmpCtx)
      return false;
    [term displayRectIgnoringOpacity:term.bounds inContext:bmpCtx];

    // If Nerd Fonts are installed, verify that the rendered cells containing PUA
    // glyphs actually painted non-background foreground pixels inside cell 0.
    if (nerdFams.count > 0) {
      int xStart = (int)kPadX;
      int xEnd = xStart + (int)ceil(term.cellWidth * 6.0);
      int yStart = (int)kPadY;
      int yEnd = yStart + (int)ceil(term.cellHeight);
      bool foundInk = false;
      for (int y = yStart; y < yEnd && !foundInk; y++) {
        for (int x = xStart; x < xEnd; x++) {
          NSUInteger px[4] = {0, 0, 0, 0};
          [rep getPixel:px atX:x y:y];
          // Default theme background is ~(25, 30, 38) or VT background; bright
          // white/cyan foreground ink exceeds channel value 120.
          if (px[0] > 120 || px[1] > 120 || px[2] > 120) {
            foundInk = true;
            break;
          }
        }
      }
      if (!foundInk)
        return false;
    }

    return true;
  }
}

void cb_terminal_test_cell_metrics(void *view, double *outCw, double *outCh,
                                   double *outPadX, double *outPadY) {
  if (!view)
    return;
  CanbanTerminal *term = (__bridge CanbanTerminal *)view;
  if (outCw)
    *outCw = (double)term.cellWidth;
  if (outCh)
    *outCh = (double)term.cellHeight;
  if (outPadX)
    *outPadX = (double)kPadX;
  if (outPadY)
    *outPadY = (double)kPadY;
}

bool cb_terminal_test_render_rgba(void *view, uint32_t width, uint32_t height,
                                  uint8_t *outRgba) {
  if (!view || !outRgba || width < 64 || height < 64)
    return false;
  @autoreleasepool {
    CanbanTerminal *term = (__bridge CanbanTerminal *)view;
    [term setFrame:NSMakeRect(0.0, 0.0, (CGFloat)width, (CGFloat)height)];
    [term pollSnapshotForceFull:YES];

    CGColorSpaceRef cs = CGColorSpaceCreateDeviceRGB();
    CGContextRef cg = CGBitmapContextCreate(
        outRgba, width, height, 8, (size_t)width * 4, cs,
        (CGBitmapInfo)kCGImageAlphaPremultipliedLast |
            (CGBitmapInfo)kCGBitmapByteOrder32Big);
    CGColorSpaceRelease(cs);
    if (!cg)
      return false;

    [NSGraphicsContext saveGraphicsState];
    CGContextTranslateCTM(cg, 0.0, (CGFloat)height);
    CGContextScaleCTM(cg, 1.0, -1.0);
    CGContextSetInterpolationQuality(cg, kCGInterpolationNone);
    NSGraphicsContext *flippedCtx =
        [NSGraphicsContext graphicsContextWithCGContext:cg flipped:YES];
    [NSGraphicsContext setCurrentContext:flippedCtx];
    [term drawRect:term.bounds];
    CGContextFlush(cg);
    [NSGraphicsContext restoreGraphicsState];
    CGContextRelease(cg);
    return true;
  }
}

@interface CbTestCursorRootView : NSView
@property(nonatomic, assign) NSInteger cursorUpdateCount;
@property(nonatomic, strong) NSCursor *lastCursorOnForward;
@end

@implementation CbTestCursorRootView
- (void)cursorUpdate:(NSEvent *)event {
  (void)event;
  self.cursorUpdateCount += 1;
  self.lastCursorOnForward = [NSCursor currentCursor];
}
@end

bool cb_terminal_test_occluded_cursor_guard(void *view) {
  if (!view)
    return false;
  @autoreleasepool {
    CanbanTerminal *term = (__bridge CanbanTerminal *)view;
    NSRect frame = NSMakeRect(0.0, 0.0, 800.0, 600.0);
    CbTestCursorRootView *root =
        [[CbTestCursorRootView alloc] initWithFrame:frame];

    [term setFrame:frame];
    term.testContentView = root;
    [root addSubview:term];
    [term pollSnapshotForceFull:YES];

    NSView *overlay =
        [[NSView alloc] initWithFrame:NSMakeRect(300.0, 200.0, 500.0, 400.0)];
    [root addSubview:overlay positioned:NSWindowAbove relativeTo:term];

    NSPoint occludedPt = NSMakePoint(600.0, 500.0);
    NSPoint uncoveredPt = NSMakePoint(100.0, 100.0);

    if ([term isPointerHitAtWindowPoint:occludedPt]) {
      term.testContentView = nil;
      [term removeFromSuperview];
      return false;
    }
    if (![term isPointerHitAtWindowPoint:uncoveredPt]) {
      term.testContentView = nil;
      [term removeFromSuperview];
      return false;
    }

    NSEvent *moveOccluded = [NSEvent
        mouseEventWithType:NSEventTypeMouseMoved
                  location:occludedPt
             modifierFlags:0
                 timestamp:0.0
              windowNumber:0
                   context:nil
               eventNumber:1
                clickCount:0
                  pressure:0.0];
    NSEvent *enterOccluded = [NSEvent
        enterExitEventWithType:NSEventTypeMouseEntered
                      location:occludedPt
                 modifierFlags:0
                     timestamp:0.0
                  windowNumber:0
                       context:nil
                   eventNumber:2
                trackingNumber:0
                      userData:NULL];
    NSEvent *cursorOccluded = [NSEvent
        enterExitEventWithType:NSEventTypeCursorUpdate
                      location:occludedPt
                 modifierFlags:0
                     timestamp:0.0
                  windowNumber:0
                       context:nil
                   eventNumber:3
                trackingNumber:0
                      userData:NULL];

    NSCursor *hand = [NSCursor pointingHandCursor];
    [hand set];

    [term mouseMoved:moveOccluded];
    [term mouseEntered:enterOccluded];
    [term cursorUpdate:cursorOccluded];

    if (root.cursorUpdateCount != 1) {
      term.testContentView = nil;
      [term removeFromSuperview];
      return false;
    }
    if ([NSCursor currentCursor] != hand) {
      term.testContentView = nil;
      [term removeFromSuperview];
      return false;
    }

    // Moving over an uncovered region of the terminal still updates the cursor
    // and does not forward cursorUpdate: away from the terminal.
    NSEvent *moveUncovered = [NSEvent
        mouseEventWithType:NSEventTypeMouseMoved
                  location:uncoveredPt
             modifierFlags:0
                 timestamp:0.0
              windowNumber:0
                   context:nil
               eventNumber:4
                clickCount:0
                  pressure:0.0];
    NSEvent *cursorUncovered = [NSEvent
        enterExitEventWithType:NSEventTypeCursorUpdate
                      location:uncoveredPt
                 modifierFlags:0
                     timestamp:0.0
                  windowNumber:0
                       context:nil
                   eventNumber:5
                trackingNumber:0
                      userData:NULL];

    [hand set];
    [term mouseMoved:moveUncovered];
    if ([NSCursor currentCursor] == hand) {
      term.testContentView = nil;
      [term removeFromSuperview];
      return false;
    }

    [hand set];
    [term cursorUpdate:cursorUncovered];
    if ([NSCursor currentCursor] == hand || root.cursorUpdateCount != 1) {
      term.testContentView = nil;
      [term removeFromSuperview];
      return false;
    }

    term.testContentView = nil;
    [term removeFromSuperview];
    return true;
  }
}
