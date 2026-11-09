#import <Cocoa/Cocoa.h>
#import <QuartzCore/QuartzCore.h>
#include <stdint.h>
typedef const char *(*FrameFn)(void *, uint16_t, uint16_t);
typedef void (*InputFn)(void *, const char *);
@interface CanbanTerminal : NSView <NSTextInputClient>
@property void *host;
@property FrameFn frameFn;
@property InputFn inputFn;
@property NSDictionary *frameData;
@property NSTimer *timer;
@property NSString *marked;
@property NSRange markedSelection;
@property NSPoint selectionStart;
@property NSPoint selectionEnd;
@property BOOL selecting;
@property BOOL selectAll;
@property BOOL autoFocus;
@property BOOL inputSuspended;
@property NSSet<NSString *> *uiCommandKeys;
@property id keyMonitor;
@property BOOL accessibilityDirty;
@property NSTimeInterval accessibilityUpdatedAt;
@property NSRect lastWindowRect;
@property NSRect lastVisibleRect;
@property NSRect lastLayerRect;
@property NSInteger pendingRedraws;
@property double scrollRemainder;
@end
@implementation CanbanTerminal
- (void)viewDidMoveToWindow {
  [super viewDidMoveToWindow];
  if (self.autoFocus && self.window)
    [self.window makeFirstResponder:self];
}
- (BOOL)isFlipped {
  return YES;
}
- (BOOL)acceptsFirstResponder {
  return YES;
}
- (BOOL)becomeFirstResponder {
  [self emit:@{@"kind" : @"focus", @"value" : @YES}];
  return YES;
}
- (BOOL)resignFirstResponder {
  [self unmarkText];
  [self emit:@{@"kind" : @"focus", @"value" : @NO}];
  return YES;
}
- (void)emit:(NSDictionary *)value {
  NSString *kind = value[@"kind"];
  if (self.inputSuspended && ([kind isEqual:@"text"] || [kind isEqual:@"key"] || [kind isEqual:@"paste"]))
    return;
  if (!self.host)
    return;
  NSData *d = [NSJSONSerialization dataWithJSONObject:value
                                              options:0
                                                error:nil];
  NSString *s = [[NSString alloc] initWithData:d encoding:NSUTF8StringEncoding];
  self.inputFn(self.host, s.UTF8String);
}
- (void)tick {
  if (!self.host)
    return;
  NSRect position = [self convertRect:self.bounds toView:nil];
  CALayer *layer = self.layer.presentationLayer ?: self.layer;
  NSRect layerRect = [layer convertRect:layer.bounds toLayer:nil];
  NSRect visible = self.visibleRect;
  if (!NSEqualRects(position, self.lastWindowRect) ||
      !NSEqualRects(layerRect, self.lastLayerRect) ||
      !NSEqualRects(visible, self.lastVisibleRect)) {
    self.lastWindowRect = position;
    self.lastLayerRect = layerRect;
    self.lastVisibleRect = visible;
    // NSH can scroll via layer transforms before AppKit commits its bounds.
    // Redraw through the short transition so the newly exposed surface is
    // painted after the host's clipping geometry catches up.
    self.pendingRedraws = 3;
  }
  if (self.pendingRedraws > 0) {
    self.pendingRedraws--;
    [self setNeedsDisplay:YES];
  }
  const char *data =
      self.frameFn(self.host, MAX(2, (self.bounds.size.width - 12) / 8),
                   MAX(1, (self.bounds.size.height - 12) / 18));
  if (data) {
    self.frameData = [NSJSONSerialization
        JSONObjectWithData:[[NSString stringWithUTF8String:data]
                               dataUsingEncoding:NSUTF8StringEncoding]
                   options:0
                     error:nil];
    self.accessibilityDirty = YES;
    [self setNeedsDisplay:YES];
  }
  NSTimeInterval now = NSProcessInfo.processInfo.systemUptime;
  if (self.accessibilityDirty && now - self.accessibilityUpdatedAt >= 0.25) {
    self.accessibilityDirty = NO;
    self.accessibilityUpdatedAt = now;
    NSAccessibilityPostNotification(self, NSAccessibilityValueChangedNotification);
  }
}
- (NSColor *)color:(NSArray *)rgb {
  if (rgb.count != 3)
    return NSColor.textColor;
  return [NSColor colorWithSRGBRed:[rgb[0] doubleValue] / 255
                             green:[rgb[1] doubleValue] / 255
                              blue:[rgb[2] doubleValue] / 255
                             alpha:1];
}
- (BOOL)selected:(NSInteger)x row:(NSInteger)y {
  if (self.selectAll)
    return YES;
  if (!self.selecting)
    return NO;
  NSInteger a = (NSInteger)self.selectionStart.y * 100000 +
                (NSInteger)self.selectionStart.x;
  NSInteger b =
      (NSInteger)self.selectionEnd.y * 100000 + (NSInteger)self.selectionEnd.x;
  NSInteger at = y * 100000 + x;
  return at >= MIN(a, b) && at <= MAX(a, b);
}
- (void)drawRect:(NSRect)dirty {
  NSRectClip(self.bounds);
  NSDictionary *f = self.frameData;
  [[self color:f[@"background"] ?: @[ @24, @24, @28 ]] setFill];
  NSRectFill(NSIntersectionRect(dirty, self.bounds));
  NSArray *rows = f[@"rows"];
  NSInteger y = 0;
  for (NSArray *row in rows) {
    NSInteger x = 0;
    for (NSDictionary *cell in row) {
      NSRect r = NSMakeRect(6 + x * 8, 6 + y * 18, 8, 18);
      BOOL selected = [self selected:x row:y];
      [[self color:selected ? @[ @52, @87, @140 ] : cell[@"bg"]] setFill];
      NSRectFill(r);
      x++;
    }
    // Paint every background before glyphs so a wide glyph is not erased by
    // the background of its trailing cell.
    x = 0;
    for (NSDictionary *cell in row) {
      NSRect r = NSMakeRect(6 + x * 8, 6 + y * 18, 8, 18);
      NSString *text = cell[@"text"];
      if (text.length) {
        NSFont *font =
            [NSFont monospacedSystemFontOfSize:13
                                        weight:[cell[@"bold"] boolValue]
                                                   ? NSFontWeightBold
                                                   : NSFontWeightRegular];
        NSMutableDictionary *attrs = [@{
          NSFontAttributeName : font,
          NSForegroundColorAttributeName : [self color:cell[@"fg"]]
        } mutableCopy];
        if ([cell[@"underline"] boolValue])
          attrs[NSUnderlineStyleAttributeName] = @(NSUnderlineStyleSingle);
        [text drawAtPoint:r.origin withAttributes:attrs];
      }
      x++;
    }
    y++;
  }
  NSArray *cursor =
      [f[@"cursor"] isKindOfClass:NSArray.class] ? f[@"cursor"] : @[];
  if (cursor.count == 2) {
    NSPoint p = NSMakePoint(6 + [cursor[0] integerValue] * 8,
                            6 + [cursor[1] integerValue] * 18);
    [[self color:f[@"cursor_color"] ?: @[ @128, @170, @255 ]] setFill];
    NSRectFill(NSMakeRect(p.x, p.y + 16, 8, 2));
    if (self.marked.length)
      [self.marked drawAtPoint:p
                withAttributes:@{
                  NSFontAttributeName :
                      [NSFont monospacedSystemFontOfSize:13
                                                  weight:NSFontWeightRegular],
                  NSForegroundColorAttributeName : NSColor.whiteColor,
                  NSBackgroundColorAttributeName : NSColor.darkGrayColor,
                  NSUnderlineStyleAttributeName : @1
                }];
  }
  NSString *status = f[@"status"];
  if (status.length)
    [status drawAtPoint:NSMakePoint(6, MAX(6, self.bounds.size.height - 20))
         withAttributes:@{
           NSFontAttributeName : [NSFont systemFontOfSize:11],
           NSForegroundColorAttributeName : NSColor.systemOrangeColor
         }];
}
- (NSPoint)cellAt:(NSEvent *)event {
  NSPoint p = [self convertPoint:event.locationInWindow fromView:nil];
  return NSMakePoint(MAX(0, (p.x - 6) / 8), MAX(0, (p.y - 6) / 18));
}
- (void)mouseDown:(NSEvent *)event {
  [self.window makeFirstResponder:self];
  self.selectAll = NO;
  self.selecting = YES;
  self.selectionStart = [self cellAt:event];
  self.selectionEnd = self.selectionStart;
  [self setNeedsDisplay:YES];
}
- (void)mouseDragged:(NSEvent *)event {
  self.selectionEnd = [self cellAt:event];
  [self setNeedsDisplay:YES];
}
- (void)mouseUp:(NSEvent *)event {
  self.selectionEnd = [self cellAt:event];
  [self setNeedsDisplay:YES];
}
- (void)scrollWheel:(NSEvent *)event {
  // AppKit's upward wheel delta is positive; libghostty's is negative.
  // Accumulate precise trackpad deltas instead of losing fractional rows.
  self.scrollRemainder -= event.scrollingDeltaY *
      (event.hasPreciseScrollingDeltas ? 1.0 / 18.0 : 3.0);
  NSInteger rows = (NSInteger)self.scrollRemainder;
  self.scrollRemainder -= rows;
  if (rows != 0) {
    [self emit:@{@"kind" : @"scroll", @"delta" : @(rows)}];
    [self tick];
  }
}
- (NSString *)selectedText {
  NSMutableString *out = [NSMutableString string];
  NSInteger y = 0;
  for (NSArray *row in self.frameData[@"rows"]) {
    NSMutableString *line = [NSMutableString string];
    NSInteger x = 0;
    for (NSDictionary *cell in row) {
      if ([self selected:x row:y] && ![cell[@"tail"] boolValue]) {
        NSString *t = cell[@"text"];
        [line appendString:t.length ? t : @" "];
      }
      x++;
    }
    if (line.length) {
      if (out.length)
        [out appendString:@"\n"];
      [out appendString:line];
    }
    y++;
  }
  return out;
}
- (void)copy:(id)sender {
  NSString *text = [self selectedText];
  if (text.length) {
    [NSPasteboard.generalPasteboard clearContents];
    [NSPasteboard.generalPasteboard setString:text
                                      forType:NSPasteboardTypeString];
  }
}
- (void)paste:(id)sender {
  NSString *text =
      [NSPasteboard.generalPasteboard stringForType:NSPasteboardTypeString];
  if (text) {
    self.selecting = NO;
    self.selectAll = NO;
    [self emit:@{@"kind" : @"paste", @"text" : text}];
    [self setNeedsDisplay:YES];
  }
}
- (void)selectAll:(id)sender {
  self.selectAll = YES;
  [self setNeedsDisplay:YES];
}
- (BOOL)performKeyEquivalent:(NSEvent *)event {
  if (event.modifierFlags & NSEventModifierFlagCommand) {
    NSString *key = event.charactersIgnoringModifiers.lowercaseString;
    if ([key isEqual:@"c"]) {
      [self copy:nil];
      return YES;
    }
    if ([key isEqual:@"v"]) {
      [self paste:nil];
      return YES;
    }
    if ([key isEqual:@"a"]) {
      [self selectAll:nil];
      return YES;
    }
  }
  return [super performKeyEquivalent:event];
}
- (void)keyDown:(NSEvent *)event {
  if ([self performKeyEquivalent:event])
    return;
  self.selecting = NO;
  self.selectAll = NO;
  BOOL special = event.keyCode == 53 || event.keyCode == 36 ||
                 event.keyCode == 48 || event.keyCode == 51 ||
                 event.keyCode == 117 || event.keyCode >= 115 ||
                 (event.modifierFlags & NSEventModifierFlagFunction);
  if (![self hasMarkedText] &&
      (special || (event.modifierFlags & NSEventModifierFlagControl))) {
    [self emit:@{
      @"kind" : @"key",
      @"code" : @(event.keyCode),
      @"mods" : @(event.modifierFlags),
      @"text" : event.charactersIgnoringModifiers ?: @""
    }];
    return;
  }
  [self interpretKeyEvents:@[ event ]];
}
- (void)insertText:(id)text replacementRange:(NSRange)range {
  NSString *value =
      [text isKindOfClass:NSAttributedString.class] ? [text string] : text;
  [self unmarkText];
  [self emit:@{@"kind" : @"text", @"text" : value ?: @""}];
}
- (void)setMarkedText:(id)text
        selectedRange:(NSRange)selected
     replacementRange:(NSRange)range {
  self.marked =
      [text isKindOfClass:NSAttributedString.class] ? [text string] : text;
  self.markedSelection = selected;
  [self setNeedsDisplay:YES];
}
- (void)unmarkText {
  self.marked = @"";
  [self setNeedsDisplay:YES];
}
- (BOOL)hasMarkedText {
  return self.marked.length > 0;
}
- (NSRange)markedRange {
  return self.marked.length ? NSMakeRange(0, self.marked.length)
                            : NSMakeRange(NSNotFound, 0);
}
- (NSRange)selectedRange {
  return self.marked.length ? self.markedSelection : NSMakeRange(0, 0);
}
- (NSArray<NSAttributedStringKey> *)validAttributesForMarkedText {
  return @[];
}
- (NSAttributedString *)attributedSubstringForProposedRange:(NSRange)range
                                                actualRange:
                                                    (NSRangePointer)actual {
  if (actual)
    *actual = NSMakeRange(NSNotFound, 0);
  return nil;
}
- (NSUInteger)characterIndexForPoint:(NSPoint)p {
  return NSNotFound;
}
- (NSRect)firstRectForCharacterRange:(NSRange)range
                         actualRange:(NSRangePointer)actual {
  if (actual)
    *actual = NSMakeRange(0, 0);
  NSArray *c = [self.frameData[@"cursor"] isKindOfClass:NSArray.class]
                   ? self.frameData[@"cursor"]
                   : @[];
  NSRect r = NSMakeRect(6 + (c.count > 0 ? [c[0] doubleValue] : 0) * 8,
                        6 + (c.count > 1 ? [c[1] doubleValue] : 0) * 18, 8, 18);
  return [self.window convertRectToScreen:[self convertRect:r toView:nil]];
}
- (void)doCommandBySelector:(SEL)selector {
  NSString *name = NSStringFromSelector(selector);
  NSDictionary *codes = @{
    @"insertNewline:" : @36,
    @"insertTab:" : @48,
    @"deleteBackward:" : @51,
    @"moveUp:" : @126,
    @"moveDown:" : @125,
    @"moveLeft:" : @123,
    @"moveRight:" : @124,
    @"cancelOperation:" : @53
  };
  NSNumber *code = codes[name];
  if (code)
    [self
        emit:@{@"kind" : @"key", @"code" : code, @"mods" : @0, @"text" : @""}];
}
- (BOOL)isAccessibilityElement {
  return YES;
}
- (NSString *)accessibilityRole {
  return NSAccessibilityTextAreaRole;
}
- (NSString *)accessibilityLabel {
  return @"Terminal";
}
- (id)accessibilityValue {
  NSMutableString *s = [NSMutableString string];
  for (NSArray *row in self.frameData[@"rows"]) {
    for (NSDictionary *cell in row)
      [s appendString:cell[@"text"] ?: @""];
    [s appendString:@"\n"];
  }
  NSString *status = self.frameData[@"status"];
  if (status.length)
    [s appendFormat:@"\n%@", status];
  return s;
}
@end
void *cb_terminal_new(void *host, FrameFn frame, InputFn input) {
  CanbanTerminal *view =
      [[CanbanTerminal alloc] initWithFrame:NSMakeRect(0, 0, 800, 540)];
  // Keep the embedded native surface above NSH's canvas when scrolled into
  // view; non-layer-backed cached contents may otherwise be painted over.
  view.wantsLayer = YES;
  view.layer.masksToBounds = YES;
  view.host = host;
  view.frameFn = frame;
  view.inputFn = input;
  view.marked = @"";
  __weak CanbanTerminal *weak = view;
  // Observe UI shortcuts before the asynchronous engine mounts its next focus
  // target. The existing window/keymap still owns command handling.
  view.keyMonitor = [NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskKeyDown
      handler:^NSEvent *(NSEvent *event) {
        CanbanTerminal *terminal = weak;
        if (terminal && event.window == terminal.window && terminal.window.firstResponder == terminal &&
            (event.modifierFlags & NSEventModifierFlagCommand) &&
            [terminal.uiCommandKeys containsObject:event.charactersIgnoringModifiers.lowercaseString])
          terminal.inputSuspended = YES;
        return event;
      }];
  view.timer = [NSTimer timerWithTimeInterval:0.033
                                      repeats:YES
                                        block:^(NSTimer *timer) {
                                          (void)timer;
                                          [weak tick];
                                        }];
  [[NSRunLoop mainRunLoop] addTimer:view.timer forMode:NSRunLoopCommonModes];
  return (__bridge_retained void *)view;
}
void cb_terminal_detach(void *ptr) {
  CanbanTerminal *view = (__bridge CanbanTerminal *)ptr;
  view.host = NULL;
  if (view.keyMonitor) [NSEvent removeMonitor:view.keyMonitor];
  view.keyMonitor = nil;
  [view.timer invalidate];
  view.timer = nil;
}

void cb_terminal_auto_focus(void *ptr, bool enabled) {
  CanbanTerminal *view = (__bridge CanbanTerminal *)ptr;
  view.autoFocus = enabled;
  view.inputSuspended = !enabled;
  if (enabled && view.window)
    [view.window makeFirstResponder:view];
}

void cb_terminal_ui_command_keys(void *ptr, const char *json) {
  CanbanTerminal *view = (__bridge CanbanTerminal *)ptr;
  NSData *data = [[NSString stringWithUTF8String:json] dataUsingEncoding:NSUTF8StringEncoding];
  NSArray *keys = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
  view.uiCommandKeys = [NSSet setWithArray:keys ?: @[]];
}
