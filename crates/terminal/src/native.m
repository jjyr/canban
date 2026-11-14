#import <Cocoa/Cocoa.h>
#import <CoreGraphics/CoreGraphics.h>
#import <CoreText/CoreText.h>
#import <IOKit/hidsystem/IOLLEvent.h>
#import <QuartzCore/QuartzCore.h>
#include <math.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include <wchar.h>

typedef struct {
  uint32_t cp;
  uint32_t utf16_offset;
  uint16_t utf16_len;
  uint16_t flags;
  uint8_t fg[3];
  uint8_t bg[3];
  uint8_t ul_color[3];
  uint8_t has_ul_color;
  uint8_t wide;
  uint8_t selected;
} CbRenderCell;

typedef struct {
  uint16_t x;
  uint16_t y;
  uint8_t visible;
  uint8_t blinking;
  uint8_t wide_tail;
  uint8_t style;
  uint8_t color[3];
  uint8_t text_color[3];
} CbRenderCursor;

typedef struct {
  uint64_t total;
  uint64_t offset;
  uint64_t len;
} CbRenderScrollbar;

typedef struct {
  const CbRenderCell *cells;
  const uint16_t *utf16;
  const uint8_t *dirty_rows;
  const uint8_t *status_ptr;
  size_t status_len;
  uint16_t cols;
  uint16_t rows;
  uint8_t dirty_state;
  uint8_t mouse_tracking;
  uint8_t background[3];
  uint8_t foreground[3];
  CbRenderCursor cursor;
  CbRenderScrollbar scrollbar;
} CbSnapshot;

typedef struct {
  uint16_t cols;
  uint16_t rows;
  uint32_t cell_width;
  uint32_t cell_height;
  uint32_t pad_left;
  uint32_t pad_right;
  uint32_t pad_top;
  uint32_t pad_bottom;
} CbGeometry;

typedef struct {
  uint8_t action;
  uint8_t composing;
  uint8_t unidentified;
  uint16_t keycode;
  uint16_t mods;
  uint16_t consumed_mods;
  uint32_t unshifted_cp;
  const uint8_t *utf8_ptr;
  size_t utf8_len;
} CbKeyEvent;

typedef struct {
  uint8_t action;
  uint8_t button;
  uint8_t any_pressed;
  uint8_t alt_override;
  uint16_t mods;
  float x_px;
  float y_px;
} CbMouseEvent;

typedef struct {
  uint8_t kind;
  uint8_t shift_extend;
  uint8_t rectangle;
  double x_px;
  double y_px;
  uint64_t time_ns;
  uint64_t repeat_interval_ns;
} CbSelectEvent;

typedef struct {
  uint8_t found;
  uint16_t row;
  uint16_t start_col;
  uint16_t end_col;
  const uint8_t *uri_ptr;
  size_t uri_len;
} CbLinkSpan;

typedef struct {
  const CbSnapshot *(*snapshot)(void *, CbGeometry, bool);
  bool (*key)(void *, CbKeyEvent);
  void (*text)(void *, const uint8_t *, size_t);
  void (*paste)(void *, const uint8_t *, size_t);
  bool (*mouse)(void *, CbMouseEvent);
  void (*wheel)(void *, int32_t, int32_t, float, float, uint16_t, bool);
  int8_t (*select)(void *, CbSelectEvent);
  const uint8_t *(*copy_text)(void *, size_t *);
  bool (*has_selection)(void *);
  CbLinkSpan (*link_at)(void *, double, double);
  void (*focus)(void *, bool);
  void (*clear_screen)(void *);
  const uint8_t *(*accessibility_text)(void *, size_t *);
  const uint8_t *(*take_clipboard)(void *, size_t *);
  bool (*take_bell)(void *);
} CbCallbacks;

enum {
  CB_MODS_SHIFT = (1 << 0),
  CB_MODS_CTRL = (1 << 1),
  CB_MODS_ALT = (1 << 2),
  CB_MODS_SUPER = (1 << 3),
  CB_MODS_CAPS_LOCK = (1 << 4),
  CB_MODS_NUM_LOCK = (1 << 5),
  CB_MODS_SHIFT_SIDE = (1 << 6),
  CB_MODS_CTRL_SIDE = (1 << 7),
  CB_MODS_ALT_SIDE = (1 << 8),
  CB_MODS_SUPER_SIDE = (1 << 9),
};

enum {
  CB_STYLE_BOLD = (1 << 0),
  CB_STYLE_ITALIC = (1 << 1),
  CB_STYLE_FAINT = (1 << 2),
  CB_STYLE_BLINK = (1 << 3),
  CB_STYLE_INVERSE = (1 << 4),
  CB_STYLE_INVISIBLE = (1 << 5),
  CB_STYLE_STRIKETHROUGH = (1 << 6),
  CB_STYLE_OVERLINE = (1 << 7),
  CB_STYLE_UNDERLINE_SHIFT = 8,
  CB_STYLE_UNDERLINE_MASK = (0x7 << 8),
};

enum {
  CB_UNDERLINE_NONE = 0,
  CB_UNDERLINE_SINGLE = 1,
  CB_UNDERLINE_DOUBLE = 2,
  CB_UNDERLINE_CURLY = 3,
  CB_UNDERLINE_DOTTED = 4,
  CB_UNDERLINE_DASHED = 5,
};

enum {
  CB_CURSOR_BAR = 0,
  CB_CURSOR_BLOCK = 1,
  CB_CURSOR_UNDERLINE = 2,
  CB_CURSOR_BLOCK_HOLLOW = 3,
};

enum {
  CB_WIDE_NARROW = 0,
  CB_WIDE_WIDE = 1,
  CB_WIDE_SPACER_TAIL = 2,
  CB_WIDE_SPACER_HEAD = 3,
};

static const CGFloat kPadX = 8.0;
static const CGFloat kPadY = 6.0;

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

static int cb_char_cells(uint32_t cp) {
  if (cp == 0)
    return 1;
  int w = wcwidth((wchar_t)cp);
  return w == 2 ? 2 : 1;
}

static bool cb_draw_box_or_block(CGContextRef ctx, uint32_t cp, NSRect cell,
                                 CGColorRef color, CGFloat scale) {
  if (!((cp >= 0x2500 && cp <= 0x259F) || (cp >= 0x2800 && cp <= 0x28FF)))
    return false;

  CGContextSaveGState(ctx);
  CGContextSetFillColorWithColor(ctx, color);
  CGContextSetStrokeColorWithColor(ctx, color);

  CGFloat x = cell.origin.x;
  CGFloat y = cell.origin.y;
  CGFloat w = cell.size.width;
  CGFloat h = cell.size.height;
  CGFloat px = scale > 0.0 ? (1.0 / scale) : 1.0;
  CGFloat thin = fmax(px, round(fmax(1.0, w * 0.12) / px) * px);
  CGFloat heavy = fmax(thin + px, round((thin * 2.0) / px) * px);
  CGFloat cx = x + round((w * 0.5) / px) * px;
  CGFloat cy = y + round((h * 0.5) / px) * px;

  // Braille U+2800..U+28FF
  if (cp >= 0x2800 && cp <= 0x28FF) {
    uint8_t bits = (uint8_t)(cp - 0x2800);
    if (bits != 0) {
      CGFloat dotW = fmax(px, round((w * 0.22) / px) * px);
      CGFloat dotH = fmax(px, round((h * 0.14) / px) * px);
      CGFloat colX[2] = {x + w * 0.28 - dotW * 0.5, x + w * 0.72 - dotW * 0.5};
      CGFloat rowY[4] = {y + h * 0.15 - dotH * 0.5, y + h * 0.38 - dotH * 0.5,
                         y + h * 0.62 - dotH * 0.5, y + h * 0.85 - dotH * 0.5};
      static const int mapCol[8] = {0, 0, 0, 1, 1, 1, 0, 1};
      static const int mapRow[8] = {0, 1, 2, 0, 1, 2, 3, 3};
      for (int b = 0; b < 8; b++) {
        if (bits & (1 << b)) {
          CGRect dot = CGRectMake(colX[mapCol[b]], rowY[mapRow[b]], dotW, dotH);
          CGContextFillEllipseInRect(ctx, dot);
        }
      }
    }
    CGContextRestoreGState(ctx);
    return true;
  }

  // Block Elements U+2580..U+259F
  if (cp >= 0x2580 && cp <= 0x259F) {
    switch (cp) {
    case 0x2580: // Upper half block
      CGContextFillRect(ctx, CGRectMake(x, y, w, h * 0.5));
      break;
    case 0x2581:
    case 0x2582:
    case 0x2583:
    case 0x2584:
    case 0x2585:
    case 0x2586:
    case 0x2587:
    case 0x2588: { // Lower 1/8 .. Full block
      CGFloat frac = (CGFloat)(cp - 0x2580) / 8.0;
      CGFloat bh = h * frac;
      CGContextFillRect(ctx, CGRectMake(x, y + h - bh, w, bh));
      break;
    }
    case 0x2589:
    case 0x258A:
    case 0x258B:
    case 0x258C:
    case 0x258D:
    case 0x258E:
    case 0x258F: { // Left 7/8 .. Left 1/8
      CGFloat frac = (CGFloat)(0x2590 - cp) / 8.0;
      CGContextFillRect(ctx, CGRectMake(x, y, w * frac, h));
      break;
    }
    case 0x2590: // Right half block
      CGContextFillRect(ctx, CGRectMake(x + w * 0.5, y, w * 0.5, h));
      break;
    case 0x2591: // Light shade
      CGContextSetAlpha(ctx, 0.25);
      CGContextFillRect(ctx, CGRectMake(x, y, w, h));
      break;
    case 0x2592: // Medium shade
      CGContextSetAlpha(ctx, 0.50);
      CGContextFillRect(ctx, CGRectMake(x, y, w, h));
      break;
    case 0x2593: // Dark shade
      CGContextSetAlpha(ctx, 0.75);
      CGContextFillRect(ctx, CGRectMake(x, y, w, h));
      break;
    case 0x2594: // Upper one eighth block
      CGContextFillRect(ctx, CGRectMake(x, y, w, h / 8.0));
      break;
    case 0x2595: // Right one eighth block
      CGContextFillRect(ctx, CGRectMake(x + w * 7.0 / 8.0, y, w / 8.0, h));
      break;
    default: {
      // Quadrants U+2596..U+259F (bits: UL=1, UR=2, LL=4, LR=8)
      static const uint8_t quadMask[10] = {
          4,         // 2596 LL
          8,         // 2597 LR
          1,         // 2598 UL
          1 | 4 | 8, // 2599 UL+LL+LR
          1 | 8,     // 259A UL+LR
          1 | 2 | 4, // 259B UL+UR+LL
          1 | 2 | 8, // 259C UL+UR+LR
          2,         // 259D UR
          2 | 4,     // 259E UR+LL
          2 | 4 | 8, // 259F UR+LL+LR
      };
      uint8_t m = quadMask[cp - 0x2596];
      CGFloat hw = w * 0.5, hh = h * 0.5;
      if (m & 1)
        CGContextFillRect(ctx, CGRectMake(x, y, hw, hh));
      if (m & 2)
        CGContextFillRect(ctx, CGRectMake(x + hw, y, w - hw, hh));
      if (m & 4)
        CGContextFillRect(ctx, CGRectMake(x, y + hh, hw, h - hh));
      if (m & 8)
        CGContextFillRect(ctx, CGRectMake(x + hw, y + hh, w - hw, h - hh));
      break;
    }
    }
    CGContextRestoreGState(ctx);
    return true;
  }

  // Box Drawing U+2500..U+257F
  // Arms: 0=none, 1=light, 2=heavy, 3=double (Up, Right, Down, Left)
  uint8_t up = 0, right = 0, down = 0, left = 0;
  bool handled = true;

  if (cp == 0x2571 || cp == 0x2572 || cp == 0x2573) {
    CGContextSetLineWidth(ctx, thin);
    if (cp == 0x2571 || cp == 0x2573) {
      CGContextMoveToPoint(ctx, x + w, y);
      CGContextAddLineToPoint(ctx, x, y + h);
      CGContextStrokePath(ctx);
    }
    if (cp == 0x2572 || cp == 0x2573) {
      CGContextMoveToPoint(ctx, x, y);
      CGContextAddLineToPoint(ctx, x + w, y + h);
      CGContextStrokePath(ctx);
    }
    CGContextRestoreGState(ctx);
    return true;
  }
  if (cp >= 0x256D && cp <= 0x2570) {
    // Rounded corners: ╭ ╮ ╯ ╰
    CGContextSetLineWidth(ctx, thin);
    CGFloat r = fmin(w, h) * 0.5;
    CGContextBeginPath(ctx);
    if (cp == 0x256D) { // ╭ down + right
      CGContextMoveToPoint(ctx, cx, y + h);
      CGContextAddArcToPoint(ctx, cx, cy, x + w, cy, r);
      CGContextAddLineToPoint(ctx, x + w, cy);
    } else if (cp == 0x256E) { // ╮ down + left
      CGContextMoveToPoint(ctx, cx, y + h);
      CGContextAddArcToPoint(ctx, cx, cy, x, cy, r);
      CGContextAddLineToPoint(ctx, x, cy);
    } else if (cp == 0x256F) { // ╯ up + left
      CGContextMoveToPoint(ctx, cx, y);
      CGContextAddArcToPoint(ctx, cx, cy, x, cy, r);
      CGContextAddLineToPoint(ctx, x, cy);
    } else { // ╰ up + right
      CGContextMoveToPoint(ctx, cx, y);
      CGContextAddArcToPoint(ctx, cx, cy, x + w, cy, r);
      CGContextAddLineToPoint(ctx, x + w, cy);
    }
    CGContextStrokePath(ctx);
    CGContextRestoreGState(ctx);
    return true;
  }
  if (cp >= 0x2504 && cp <= 0x250B) {
    // Dashed horizontal / vertical
    bool vert = (cp >= 0x2506 && cp <= 0x2507) || (cp >= 0x250A && cp <= 0x250B);
    CGFloat sw = ((cp & 1) == 1) ? heavy : thin;
    int segs = (cp >= 0x2508) ? 4 : 3;
    if (!vert) {
      CGFloat step = w / segs;
      for (int i = 0; i < segs; i++) {
        CGContextFillRect(ctx, CGRectMake(x + i * step + step * 0.1, cy - sw * 0.5, step * 0.75, sw));
      }
    } else {
      CGFloat step = h / segs;
      for (int i = 0; i < segs; i++) {
        CGContextFillRect(ctx, CGRectMake(cx - sw * 0.5, y + i * step + step * 0.1, sw, step * 0.75));
      }
    }
    CGContextRestoreGState(ctx);
    return true;
  }
  if (cp >= 0x254C && cp <= 0x254F) {
    bool vert = (cp >= 0x254E);
    CGFloat sw = ((cp & 1) == 1) ? heavy : thin;
    if (!vert) {
      CGFloat step = w * 0.5;
      for (int i = 0; i < 2; i++)
        CGContextFillRect(ctx, CGRectMake(x + i * step + step * 0.1, cy - sw * 0.5, step * 0.75, sw));
    } else {
      CGFloat step = h * 0.5;
      for (int i = 0; i < 2; i++)
        CGContextFillRect(ctx, CGRectMake(cx - sw * 0.5, y + i * step + step * 0.1, sw, step * 0.75));
    }
    CGContextRestoreGState(ctx);
    return true;
  }

  // Packed URDL nibble table for standard box characters (0=none,1=light,2=heavy,3=double)
  switch (cp) {
  case 0x2500: right = 1; left = 1; break;
  case 0x2501: right = 2; left = 2; break;
  case 0x2502: up = 1; down = 1; break;
  case 0x2503: up = 2; down = 2; break;
  case 0x250C: right = 1; down = 1; break;
  case 0x250D: right = 2; down = 1; break;
  case 0x250E: right = 1; down = 2; break;
  case 0x250F: right = 2; down = 2; break;
  case 0x2510: down = 1; left = 1; break;
  case 0x2511: down = 1; left = 2; break;
  case 0x2512: down = 2; left = 1; break;
  case 0x2513: down = 2; left = 2; break;
  case 0x2514: up = 1; right = 1; break;
  case 0x2515: up = 1; right = 2; break;
  case 0x2516: up = 2; right = 1; break;
  case 0x2517: up = 2; right = 2; break;
  case 0x2518: up = 1; left = 1; break;
  case 0x2519: up = 1; left = 2; break;
  case 0x251A: up = 2; left = 1; break;
  case 0x251B: up = 2; left = 2; break;
  case 0x251C: up = 1; right = 1; down = 1; break;
  case 0x251D: up = 1; right = 2; down = 1; break;
  case 0x251E: up = 2; right = 1; down = 1; break;
  case 0x251F: up = 1; right = 1; down = 2; break;
  case 0x2520: up = 2; right = 1; down = 2; break;
  case 0x2521: up = 2; right = 2; down = 1; break;
  case 0x2522: up = 1; right = 2; down = 2; break;
  case 0x2523: up = 2; right = 2; down = 2; break;
  case 0x2524: up = 1; down = 1; left = 1; break;
  case 0x2525: up = 1; down = 1; left = 2; break;
  case 0x2526: up = 2; down = 1; left = 1; break;
  case 0x2527: up = 1; down = 2; left = 1; break;
  case 0x2528: up = 2; down = 2; left = 1; break;
  case 0x2529: up = 2; down = 1; left = 2; break;
  case 0x252A: up = 1; down = 2; left = 2; break;
  case 0x252B: up = 2; down = 2; left = 2; break;
  case 0x252C: right = 1; down = 1; left = 1; break;
  case 0x252D: right = 1; down = 1; left = 2; break;
  case 0x252E: right = 2; down = 1; left = 1; break;
  case 0x252F: right = 2; down = 1; left = 2; break;
  case 0x2530: right = 1; down = 2; left = 1; break;
  case 0x2531: right = 1; down = 2; left = 2; break;
  case 0x2532: right = 2; down = 2; left = 1; break;
  case 0x2533: right = 2; down = 2; left = 2; break;
  case 0x2534: up = 1; right = 1; left = 1; break;
  case 0x2535: up = 1; right = 1; left = 2; break;
  case 0x2536: up = 1; right = 2; left = 1; break;
  case 0x2537: up = 1; right = 2; left = 2; break;
  case 0x2538: up = 2; right = 1; left = 1; break;
  case 0x2539: up = 2; right = 1; left = 2; break;
  case 0x253A: up = 2; right = 2; left = 1; break;
  case 0x253B: up = 2; right = 2; left = 2; break;
  case 0x253C: up = 1; right = 1; down = 1; left = 1; break;
  case 0x253D: up = 1; right = 1; down = 1; left = 2; break;
  case 0x253E: up = 1; right = 2; down = 1; left = 1; break;
  case 0x253F: up = 1; right = 2; down = 1; left = 2; break;
  case 0x2540: up = 2; right = 1; down = 1; left = 1; break;
  case 0x2541: up = 1; right = 1; down = 2; left = 1; break;
  case 0x2542: up = 2; right = 1; down = 2; left = 1; break;
  case 0x2543: up = 2; right = 1; down = 1; left = 2; break;
  case 0x2544: up = 2; right = 2; down = 1; left = 1; break;
  case 0x2545: up = 1; right = 1; down = 2; left = 2; break;
  case 0x2546: up = 1; right = 2; down = 2; left = 1; break;
  case 0x2547: up = 2; right = 2; down = 1; left = 2; break;
  case 0x2548: up = 1; right = 2; down = 2; left = 2; break;
  case 0x2549: up = 2; right = 1; down = 2; left = 2; break;
  case 0x254A: up = 2; right = 2; down = 2; left = 1; break;
  case 0x254B: up = 2; right = 2; down = 2; left = 2; break;
  case 0x2550: right = 3; left = 3; break;
  case 0x2551: up = 3; down = 3; break;
  case 0x2552: right = 3; down = 1; break;
  case 0x2553: right = 1; down = 3; break;
  case 0x2554: right = 3; down = 3; break;
  case 0x2555: down = 1; left = 3; break;
  case 0x2556: down = 3; left = 1; break;
  case 0x2557: down = 3; left = 3; break;
  case 0x2558: up = 1; right = 3; break;
  case 0x2559: up = 3; right = 1; break;
  case 0x255A: up = 3; right = 3; break;
  case 0x255B: up = 1; left = 3; break;
  case 0x255C: up = 3; left = 1; break;
  case 0x255D: up = 3; left = 3; break;
  case 0x255E: up = 1; right = 3; down = 1; break;
  case 0x255F: up = 3; right = 1; down = 3; break;
  case 0x2560: up = 3; right = 3; down = 3; break;
  case 0x2561: up = 1; down = 1; left = 3; break;
  case 0x2562: up = 3; down = 3; left = 1; break;
  case 0x2563: up = 3; down = 3; left = 3; break;
  case 0x2564: right = 3; down = 1; left = 3; break;
  case 0x2565: right = 1; down = 3; left = 1; break;
  case 0x2566: right = 3; down = 3; left = 3; break;
  case 0x2567: up = 1; right = 3; left = 3; break;
  case 0x2568: up = 3; right = 1; left = 1; break;
  case 0x2569: up = 3; right = 3; left = 3; break;
  case 0x256A: up = 1; right = 3; down = 1; left = 3; break;
  case 0x256B: up = 3; right = 1; down = 3; left = 1; break;
  case 0x256C: up = 3; right = 3; down = 3; left = 3; break;
  case 0x2574: left = 1; break;
  case 0x2575: up = 1; break;
  case 0x2576: right = 1; break;
  case 0x2577: down = 1; break;
  case 0x2578: left = 2; break;
  case 0x2579: up = 2; break;
  case 0x257A: right = 2; break;
  case 0x257B: down = 2; break;
  case 0x257C: left = 1; right = 2; break;
  case 0x257D: up = 1; down = 2; break;
  case 0x257E: left = 2; right = 1; break;
  case 0x257F: up = 2; down = 1; break;
  default: handled = false; break;
  }

  if (!handled) {
    CGContextRestoreGState(ctx);
    return false;
  }

  CGFloat gap = fmax(thin * 1.5, 2.0 * px);
  // Draw single/heavy arms reaching the center with half-stroke overlap so corners join seamlessly.
  CGFloat maxV = (up == 2 || down == 2) ? heavy : thin;
  CGFloat maxH = (left == 2 || right == 2) ? heavy : thin;

  if (up == 1 || up == 2) {
    CGFloat sw = (up == 2) ? heavy : thin;
    CGContextFillRect(ctx, CGRectMake(cx - sw * 0.5, y, sw, (cy - y) + maxH * 0.5));
  } else if (up == 3) {
    CGFloat off = gap * 0.5 + thin * 0.5;
    CGFloat endL = (left == 3) ? (cy - gap * 0.5 + thin * 0.5) : (cy + thin * 0.5);
    CGFloat endR = (right == 3) ? (cy - gap * 0.5 + thin * 0.5) : (cy + thin * 0.5);
    CGContextFillRect(ctx, CGRectMake(cx - off - thin * 0.5, y, thin, endL - y));
    CGContextFillRect(ctx, CGRectMake(cx + off - thin * 0.5, y, thin, endR - y));
  }

  if (down == 1 || down == 2) {
    CGFloat sw = (down == 2) ? heavy : thin;
    CGFloat startY = cy - maxH * 0.5;
    CGContextFillRect(ctx, CGRectMake(cx - sw * 0.5, startY, sw, (y + h) - startY));
  } else if (down == 3) {
    CGFloat off = gap * 0.5 + thin * 0.5;
    CGFloat startL = (left == 3) ? (cy + gap * 0.5 - thin * 0.5) : (cy - thin * 0.5);
    CGFloat startR = (right == 3) ? (cy + gap * 0.5 - thin * 0.5) : (cy - thin * 0.5);
    CGContextFillRect(ctx, CGRectMake(cx - off - thin * 0.5, startL, thin, (y + h) - startL));
    CGContextFillRect(ctx, CGRectMake(cx + off - thin * 0.5, startR, thin, (y + h) - startR));
  }

  if (left == 1 || left == 2) {
    CGFloat sw = (left == 2) ? heavy : thin;
    CGContextFillRect(ctx, CGRectMake(x, cy - sw * 0.5, (cx - x) + maxV * 0.5, sw));
  } else if (left == 3) {
    CGFloat off = gap * 0.5 + thin * 0.5;
    CGFloat endT = (up == 3) ? (cx - gap * 0.5 + thin * 0.5) : (cx + thin * 0.5);
    CGFloat endB = (down == 3) ? (cx - gap * 0.5 + thin * 0.5) : (cx + thin * 0.5);
    CGContextFillRect(ctx, CGRectMake(x, cy - off - thin * 0.5, endT - x, thin));
    CGContextFillRect(ctx, CGRectMake(x, cy + off - thin * 0.5, endB - x, thin));
  }

  if (right == 1 || right == 2) {
    CGFloat sw = (right == 2) ? heavy : thin;
    CGFloat startX = cx - maxV * 0.5;
    CGContextFillRect(ctx, CGRectMake(startX, cy - sw * 0.5, (x + w) - startX, sw));
  } else if (right == 3) {
    CGFloat off = gap * 0.5 + thin * 0.5;
    CGFloat startT = (up == 3) ? (cx + gap * 0.5 - thin * 0.5) : (cx - thin * 0.5);
    CGFloat startB = (down == 3) ? (cx + gap * 0.5 - thin * 0.5) : (cx - thin * 0.5);
    CGContextFillRect(ctx, CGRectMake(startT, cy - off - thin * 0.5, (x + w) - startT, thin));
    CGContextFillRect(ctx, CGRectMake(startB, cy + off - thin * 0.5, (x + w) - startB, thin));
  }

  CGContextRestoreGState(ctx);
  return true;
}

static void cb_register_user_font_subdirs_once(void) {
  static dispatch_once_t onceToken;
  dispatch_once(&onceToken, ^{
    NSString *home = NSHomeDirectory();
    if (!home || home.length == 0)
      return;
    NSString *fontsRoot = [home stringByAppendingPathComponent:@"Library/Fonts"];
    NSURL *rootURL = [NSURL fileURLWithPath:fontsRoot isDirectory:YES];
    NSFileManager *fm = [NSFileManager defaultManager];
    NSDirectoryEnumerator<NSURL *> *enumerator = [fm
                   enumeratorAtURL:rootURL
        includingPropertiesForKeys:@[ NSURLIsRegularFileKey ]
                           options:NSDirectoryEnumerationSkipsHiddenFiles
                      errorHandler:nil];
    if (!enumerator)
      return;
    NSMutableArray<NSURL *> *urls = [NSMutableArray array];
    NSSet<NSString *> *exts =
        [NSSet setWithArray:@[ @"ttf", @"otf", @"ttc", @"otc" ]];
    for (NSURL *fileURL in enumerator) {
      NSString *ext = fileURL.pathExtension.lowercaseString;
      if (!ext || ![exts containsObject:ext])
        continue;
      NSURL *parent = [fileURL URLByDeletingLastPathComponent];
      if ([parent.path isEqualToString:rootURL.path])
        continue;
      [urls addObject:fileURL];
    }
    if (urls.count > 0) {
      CTFontManagerRegisterFontURLs((__bridge CFArrayRef)urls,
                                    kCTFontManagerScopeProcess, true, NULL);
    }
  });
}

static bool cb_is_ascii_monospaced(CTFontRef font) {
  if (!font)
    return false;
  // Require printable ASCII glyphs ('i', 'm', 'W', '0', '_') to exist and share
  // identical horizontal advance widths so symbol-only or proportional fonts do
  // not masquerade as primary terminal fonts.
  UniChar chars[5] = {0x0069, 0x006D, 0x0057, 0x0030, 0x005F};
  CGGlyph glyphs[5] = {0, 0, 0, 0, 0};
  if (!CTFontGetGlyphsForCharacters(font, chars, glyphs, 5))
    return false;
  for (int i = 0; i < 5; i++) {
    if (glyphs[i] == 0)
      return false;
  }
  CGSize advs[5] = {CGSizeZero};
  CTFontGetAdvancesForGlyphs(font, kCTFontOrientationHorizontal, glyphs, advs,
                             5);
  CGFloat w0 = advs[0].width;
  if (w0 < 1.0)
    return false;
  for (int i = 1; i < 5; i++) {
    if (fabs(advs[i].width - w0) > 0.05)
      return false;
  }
  return true;
}

static CTFontRef cb_create_styled_font(NSString *family, CGFloat size,
                                       CTFontSymbolicTraits traits) {
  static NSMutableDictionary<NSString *, id> *sFontCache = nil;
  static dispatch_once_t cacheOnce;
  dispatch_once(&cacheOnce, ^{
    sFontCache = [NSMutableDictionary dictionaryWithCapacity:32];
  });
  NSString *cacheKey = [NSString
      stringWithFormat:@"%@|%.2f|%u", family ?: @"<system>", size, (unsigned)traits];
  @synchronized(sFontCache) {
    id cached = sFontCache[cacheKey];
    if (cached) {
      return (CTFontRef)CFRetain((__bridge CTFontRef)cached);
    }
  }
  cb_register_user_font_subdirs_once();
  NSFont *base = nil;
  if (family && family.length > 0) {
    base = [NSFont fontWithName:family size:size];
    if (!base) {
      base = [[NSFontManager sharedFontManager] fontWithFamily:family
                                                        traits:0
                                                        weight:5
                                                          size:size];
    }
    if (!base) {
      NSFontDescriptor *desc = [NSFontDescriptor
          fontDescriptorWithFontAttributes:@{NSFontFamilyAttribute : family}];
      base = [NSFont fontWithDescriptor:desc size:size];
    }
  }
  if (!base) {
    if (@available(macOS 10.15, *)) {
      base = [NSFont monospacedSystemFontOfSize:size weight:NSFontWeightRegular];
    } else {
      base = [NSFont fontWithName:@"Menlo" size:size] ?:
             [NSFont userFixedPitchFontOfSize:size];
    }
  }
  CTFontRef ctBase = (__bridge CTFontRef)base;
  CTFontRef result = NULL;
  if (traits == 0) {
    result = (CTFontRef)CFRetain(ctBase);
  } else {
    CTFontRef styled =
        CTFontCreateCopyWithSymbolicTraits(ctBase, size, NULL, traits, traits);
    if (styled) {
      result = styled;
    } else if (traits == (kCTFontBoldTrait | kCTFontItalicTrait)) {
      CTFontRef boldOnly = CTFontCreateCopyWithSymbolicTraits(
          ctBase, size, NULL, kCTFontBoldTrait, kCTFontBoldTrait);
      if (boldOnly)
        result = boldOnly;
    }
    if (!result) {
      result = (CTFontRef)CFRetain(ctBase);
    }
  }
  if (result) {
    @synchronized(sFontCache) {
      if (sFontCache.count > 64) {
        [sFontCache removeAllObjects];
      }
      sFontCache[cacheKey] = (__bridge id)result;
    }
  }
  return result;
}

@interface CanbanTerminal : NSView <NSTextInputClient> {
@public
  void *_host;
  CbCallbacks _cb;
  const CbSnapshot *_snap;
  CTFontRef _fontRegular;
  CTFontRef _fontBold;
  CTFontRef _fontItalic;
  CTFontRef _fontBoldItalic;
  CFMutableDictionaryRef _fallbackCache;
  uint64_t _physicalKeyDownLo;
  uint64_t _physicalKeyDownHi;
  uint8_t _themeBg[3];
}
@property(nonatomic, copy) NSString *fontFamily;
@property(nonatomic) CGFloat fontSize;
@property(nonatomic) CGFloat cellWidth;
@property(nonatomic) CGFloat cellHeight;
@property(nonatomic) CGFloat baselineOffset;
@property(nonatomic) CGFloat underlinePosition;
@property(nonatomic) CGFloat underlineThickness;
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
  self = [super initWithFrame:NSMakeRect(0, 0, 640, 380)];
  if (!self)
    return nil;
  _host = host;
  _cb = callbacks;
  _snap = NULL;
  _fontFamily = nil;
  _fontSize = 13.0;
  _backingScale = 2.0;
  _uiCommandKeys = [NSSet set];
  _markedRangeVal = NSMakeRange(NSNotFound, 0);
  _selectedRangeVal = NSMakeRange(0, 0);
  _cursorBlinkVisible = YES;
  _lastBlinkTime = [NSProcessInfo processInfo].systemUptime;
  _fallbackCache = CFDictionaryCreateMutable(
      kCFAllocatorDefault, 0, NULL, &kCFTypeDictionaryValueCallBacks);
  _themeBg[0] = 25;
  _themeBg[1] = 30;
  _themeBg[2] = 38;
  self.wantsLayer = YES;
  self.layer.masksToBounds = YES;
  self.layer.opaque = YES;
  [self applyThemeBgLayerColor];
  [self rebuildFonts];
  return self;
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
  _selectingLocally = NO;
  _autoscrollDir = 0;
  _hoverLinkValid = NO;
  _hoverLinkUri = nil;
  _scrollbarVisibleUntil = 0;
  _prevScrollOffset = 0;
  _prevScrollTotal = 0;
  _cursorBlinkVisible = YES;
  _lastBlinkTime = [NSProcessInfo processInfo].systemUptime;
  _snap = NULL;
  [self pollSnapshotForceFull:YES];
  [self setNeedsDisplay:YES];
  if (_autoFocus) {
    [self requestAutoFocusIfEligible];
  }
}

- (void)releaseFonts {
  if (_fontRegular) {
    CFRelease(_fontRegular);
    _fontRegular = NULL;
  }
  if (_fontBold) {
    CFRelease(_fontBold);
    _fontBold = NULL;
  }
  if (_fontItalic) {
    CFRelease(_fontItalic);
    _fontItalic = NULL;
  }
  if (_fontBoldItalic) {
    CFRelease(_fontBoldItalic);
    _fontBoldItalic = NULL;
  }
  if (_fallbackCache) {
    CFDictionaryRemoveAllValues(_fallbackCache);
  }
}

- (void)detachHost {
  _host = NULL;
  _snap = NULL;
  [self.animTimer invalidate];
  self.animTimer = nil;
  [[NSNotificationCenter defaultCenter] removeObserver:self];
}

- (void)dealloc {
  [self detachHost];
  [self releaseFonts];
  if (_fallbackCache) {
    CFRelease(_fallbackCache);
    _fallbackCache = NULL;
  }
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
  [self releaseFonts];
  CGFloat sz = fmax(9.0, fmin(32.0, _fontSize > 0.0 ? _fontSize : 13.0));
  _fontRegular = cb_create_styled_font(_fontFamily, sz, 0);
  _fontBold = cb_create_styled_font(_fontFamily, sz, kCTFontBoldTrait);
  _fontItalic = cb_create_styled_font(_fontFamily, sz, kCTFontItalicTrait);
  _fontBoldItalic = cb_create_styled_font(
      _fontFamily, sz, kCTFontBoldTrait | kCTFontItalicTrait);

  CGFloat scale = _backingScale > 0.0 ? _backingScale : 2.0;
  CGFloat ascent = CTFontGetAscent(_fontRegular);
  CGFloat descent = CTFontGetDescent(_fontRegular);
  CGFloat leading = fmax(0.0, CTFontGetLeading(_fontRegular));

  UniChar ch = '0';
  CGGlyph glyph = 0;
  CGSize adv = CGSizeZero;
  if (!CTFontGetGlyphsForCharacters(_fontRegular, &ch, &glyph, 1)) {
    ch = 'M';
    CTFontGetGlyphsForCharacters(_fontRegular, &ch, &glyph, 1);
  }
  CTFontGetAdvancesForGlyphs(_fontRegular, kCTFontOrientationHorizontal, &glyph,
                             &adv, 1);
  CGFloat rawW = adv.width > 1.0 ? adv.width : ceil(sz * 0.6);
  CGFloat rawH = ceil(ascent + descent + leading + 2.0);
  _cellWidth = fmax(4.0, round(rawW * scale) / scale);
  _cellHeight = fmax(8.0, ceil(rawH * scale) / scale);
  CGFloat extraY = fmax(0.0, _cellHeight - (ascent + descent));
  _baselineOffset = round((ascent + extraY * 0.5) * scale) / scale;
  _underlinePosition = CTFontGetUnderlinePosition(_fontRegular);
  _underlineThickness =
      fmax(1.0 / scale, CTFontGetUnderlineThickness(_fontRegular));
}

- (CTFontRef)fontForStyleFlags:(uint16_t)flags {
  BOOL bold = (flags & CB_STYLE_BOLD) != 0;
  BOOL italic = (flags & CB_STYLE_ITALIC) != 0;
  if (bold && italic)
    return _fontBoldItalic;
  if (bold)
    return _fontBold;
  if (italic)
    return _fontItalic;
  return _fontRegular;
}

- (CTFontRef)resolveFontForUTF16:(const UniChar *)chars
                          length:(CFIndex)len
                       codepoint:(uint32_t)cp
                           flags:(uint16_t)flags {
  CTFontRef primary = [self fontForStyleFlags:flags];
  if (len <= 0 || !primary)
    return primary;
  CGGlyph testGlyphs[8];
  if (len <= 8 && CTFontGetGlyphsForCharacters(primary, chars, testGlyphs, len)) {
    BOOL allFound = YES;
    for (CFIndex i = 0; i < len; i++) {
      if (testGlyphs[i] == 0) {
        allFound = NO;
        break;
      }
    }
    if (allFound)
      return primary;
  }
  uintptr_t key =
      ((uintptr_t)cp << 2) | (uintptr_t)(flags & (CB_STYLE_BOLD | CB_STYLE_ITALIC));
  if (_fallbackCache && key != 0) {
    CTFontRef cached =
        (CTFontRef)CFDictionaryGetValue(_fallbackCache, (const void *)key);
    if (cached)
      return cached;
  }
  CFStringRef str =
      CFStringCreateWithCharactersNoCopy(kCFAllocatorDefault, chars, len,
                                         kCFAllocatorNull);
  if (!str)
    return primary;
  CTFontRef fb =
      CTFontCreateForString(primary, str, CFRangeMake(0, CFStringGetLength(str)));
  CFRelease(str);
  if (!fb)
    return primary;
  if (_fallbackCache && key != 0) {
    if (CFDictionaryGetCount(_fallbackCache) > 2048) {
      CFDictionaryRemoveAllValues(_fallbackCache);
    }
    CFDictionarySetValue(_fallbackCache, (const void *)key, fb);
  }
  CFAutorelease(fb);
  return fb;
}

- (CbGeometry)currentGeometry {
  NSRect b = self.bounds;
  CGFloat cw = _cellWidth > 0.0 ? _cellWidth : 8.0;
  CGFloat ch = _cellHeight > 0.0 ? _cellHeight : 18.0;
  CGFloat innerW = fmax(cw * 2.0, b.size.width - kPadX * 2.0);
  CGFloat innerH = fmax(ch, b.size.height - kPadY * 2.0);
  uint16_t cols = (uint16_t)fmax(2.0, fmin(500.0, floor(innerW / cw)));
  uint16_t rows = (uint16_t)fmax(1.0, fmin(200.0, floor(innerH / ch)));
  CbGeometry g;
  g.cols = cols;
  g.rows = rows;
  g.cell_width = (uint32_t)fmax(1.0, round(cw));
  g.cell_height = (uint32_t)fmax(1.0, round(ch));
  g.pad_left = (uint32_t)kPadX;
  g.pad_right = (uint32_t)kPadX;
  g.pad_top = (uint32_t)kPadY;
  g.pad_bottom = (uint32_t)kPadY;
  return g;
}

- (NSRect)rectForRow:(uint16_t)row {
  CGFloat y = kPadY + (CGFloat)row * _cellHeight;
  return NSMakeRect(0.0, y - 1.0, self.bounds.size.width, _cellHeight + 2.0);
}

- (void)pollSnapshotForceFull:(BOOL)forceFull {
  if (!_host || !_cb.snapshot)
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

  if (next->scrollbar.total > next->scrollbar.len &&
      (next->scrollbar.offset != _prevScrollOffset ||
       next->scrollbar.total != _prevScrollTotal)) {
    _scrollbarVisibleUntil = [NSProcessInfo processInfo].systemUptime + 0.9;
  }
  _prevScrollOffset = next->scrollbar.offset;
  _prevScrollTotal = next->scrollbar.total;

  if (forceFull || next->dirty_state == 2) {
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
  BOOL needed =
      needCursorBlink || needAutoscroll || needScrollbarFade || needLayoutSettle;

  NSTimeInterval desiredInterval = needAutoscroll ? 0.033 : 0.08;
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
    [self.inputContext discardMarkedText];
    [self unmarkText];
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
    if (scale > 0.0 && fabs(scale - _backingScale) > 0.01) {
      _backingScale = scale;
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
  if (scale > 0.0 && fabs(scale - _backingScale) > 0.01) {
    _backingScale = scale;
    [self rebuildFonts];
    [self pollSnapshotForceFull:YES];
  }
}

- (void)setFrameSize:(NSSize)newSize {
  [super setFrameSize:newSize];
  _pendingLayoutRedraws = 0;
  [self pollSnapshotForceFull:YES];
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
    [self.inputContext discardMarkedText];
    [self unmarkText];
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

- (void)drawUnderlineInContext:(CGContextRef)ctx
                         style:(int)ulStyle
                             x:(CGFloat)x
                       rowTopY:(CGFloat)rowY
                         width:(CGFloat)width
                         color:(CGColorRef)color {
  if (ulStyle == CB_UNDERLINE_NONE || width <= 0.0)
    return;
  CGFloat scale = _backingScale > 0.0 ? _backingScale : 2.0;
  CGFloat px = 1.0 / scale;
  CGFloat thick = fmax(px, round(_underlineThickness / px) * px);
  CGFloat baseLineY = rowY + _baselineOffset;
  CGFloat ulY =
      fmin(rowY + _cellHeight - thick,
           round((baseLineY - _underlinePosition) / px) * px);

  CGContextSaveGState(ctx);
  CGContextSetStrokeColorWithColor(ctx, color);
  CGContextSetFillColorWithColor(ctx, color);
  CGContextSetLineWidth(ctx, thick);

  switch (ulStyle) {
  case CB_UNDERLINE_SINGLE:
    CGContextFillRect(ctx, CGRectMake(x, ulY, width, thick));
    break;
  case CB_UNDERLINE_DOUBLE: {
    CGFloat y1 = fmax(rowY, ulY - thick * 1.5);
    CGFloat y2 = fmin(rowY + _cellHeight - thick, ulY + thick);
    CGContextFillRect(ctx, CGRectMake(x, y1, width, thick));
    CGContextFillRect(ctx, CGRectMake(x, y2, width, thick));
    break;
  }
  case CB_UNDERLINE_DOTTED: {
    CGFloat dash[2] = {thick, fmax(px, thick * 1.5)};
    CGContextSetLineDash(ctx, 0.0, dash, 2);
    CGContextMoveToPoint(ctx, x, ulY + thick * 0.5);
    CGContextAddLineToPoint(ctx, x + width, ulY + thick * 0.5);
    CGContextStrokePath(ctx);
    break;
  }
  case CB_UNDERLINE_DASHED: {
    CGFloat dash[2] = {fmax(2.0 * px, width * 0.4), fmax(px, width * 0.25)};
    CGContextSetLineDash(ctx, 0.0, dash, 2);
    CGContextMoveToPoint(ctx, x, ulY + thick * 0.5);
    CGContextAddLineToPoint(ctx, x + width, ulY + thick * 0.5);
    CGContextStrokePath(ctx);
    break;
  }
  case CB_UNDERLINE_CURLY: {
    CGFloat amp = fmax(px, fmin(2.5, (_cellHeight - _baselineOffset) * 0.4));
    CGFloat center = fmin(rowY + _cellHeight - amp - px * 0.5, ulY);
    CGFloat period = fmax(4.0, _cellWidth);
    CGContextBeginPath(ctx);
    int steps = (int)ceil(width * scale);
    for (int s = 0; s <= steps; s++) {
      CGFloat dx = fmin(width, (CGFloat)s / scale);
      CGFloat py = center + sin(((x + dx) / period) * M_PI * 2.0) * amp;
      if (s == 0)
        CGContextMoveToPoint(ctx, x + dx, py);
      else
        CGContextAddLineToPoint(ctx, x + dx, py);
    }
    CGContextStrokePath(ctx);
    break;
  }
  default:
    break;
  }
  CGContextRestoreGState(ctx);
}

- (void)drawRect:(NSRect)dirtyRect {
  NSRectClip(self.bounds);
  dirtyRect = NSIntersectionRect(dirtyRect, self.bounds);
  if (NSIsEmptyRect(dirtyRect)) {
    return;
  }
  if (!_snap && _host) {
    [self pollSnapshotForceFull:YES];
  }
  CGContextRef ctx = [[NSGraphicsContext currentContext] CGContext];
  if (!ctx)
    return;

  const CbSnapshot *snap = _snap;
  uint8_t bgR = snap ? snap->background[0] : _themeBg[0];
  uint8_t bgG = snap ? snap->background[1] : _themeBg[1];
  uint8_t bgB = snap ? snap->background[2] : _themeBg[2];
  CGColorSpaceRef cs = CGColorSpaceCreateDeviceRGB();

  CGFloat defaultBgComp[4] = {bgR / 255.0, bgG / 255.0, bgB / 255.0, 1.0};
  CGColorRef defaultBg = CGColorCreate(cs, defaultBgComp);
  CGContextSetFillColorWithColor(ctx, defaultBg);
  CGContextFillRect(ctx, dirtyRect);
  CFRelease(defaultBg);

  if (!snap || !snap->cells || snap->cols == 0 || snap->rows == 0) {
    CGColorSpaceRelease(cs);
    return;
  }

  CGContextSetShouldSmoothFonts(ctx, true);
  CGContextSetAllowsFontSmoothing(ctx, true);
  CGContextSetShouldSubpixelPositionFonts(ctx, true);

  CGFloat cw = _cellWidth;
  CGFloat ch = _cellHeight;
  uint16_t cols = snap->cols;
  uint16_t rows = snap->rows;

  int firstRow =
      (int)fmax(0.0, floor((dirtyRect.origin.y - kPadY - 2.0) / ch));
  int lastRow = (int)fmin(
      (double)(rows - 1),
      ceil((NSMaxY(dirtyRect) - kPadY + 2.0) / ch));
  if (firstRow > lastRow) {
    firstRow = 0;
    lastRow = -1;
  }

  BOOL cursorSolidBlock = NO;
  uint16_t curX = snap->cursor.x;
  uint16_t curY = snap->cursor.y;
  int curCells = 1;
  BOOL showCursor = snap->cursor.visible &&
                    (!_focusedState || !snap->cursor.blinking || _cursorBlinkVisible);
  if (showCursor && curY < rows && curX < cols) {
    const CbRenderCell *cc = &snap->cells[(size_t)curY * cols + curX];
    if (cc->wide == CB_WIDE_WIDE && curX + 1 < cols) {
      curCells = 2;
    }
    if (_focusedState && snap->cursor.style == CB_CURSOR_BLOCK &&
        ![self hasMarkedText]) {
      cursorSolidBlock = YES;
    }
  }

  CGFloat selR = (snap->foreground[0] * 0.28 + bgR * 0.72) / 255.0;
  CGFloat selG = (snap->foreground[1] * 0.28 + bgG * 0.72) / 255.0;
  CGFloat selB = (snap->foreground[2] * 0.35 + bgB * 0.65) / 255.0;
  CGFloat selComp[4] = {selR, selG, selB, 1.0};
  CGColorRef selBg = CGColorCreate(cs, selComp);

  // Pass 1: Cell backgrounds & selection highlights (run-coalesced per row).
  for (int r = firstRow; r <= lastRow; r++) {
    CGFloat rowY = kPadY + (CGFloat)r * ch;
    const CbRenderCell *rowCells = &snap->cells[(size_t)r * cols];
    uint16_t c = 0;
    while (c < cols) {
      const CbRenderCell *cell = &rowCells[c];
      BOOL sel = (cell->selected != 0);
      BOOL nonDefaultBg =
          (cell->bg[0] != bgR || cell->bg[1] != bgG || cell->bg[2] != bgB);
      if (!sel && !nonDefaultBg) {
        c++;
        continue;
      }
      uint16_t runStart = c;
      uint8_t r0 = cell->bg[0], g0 = cell->bg[1], b0 = cell->bg[2];
      c++;
      while (c < cols) {
        const CbRenderCell *nc = &rowCells[c];
        if ((nc->selected != 0) != sel)
          break;
        if (!sel &&
            (nc->bg[0] != r0 || nc->bg[1] != g0 || nc->bg[2] != b0))
          break;
        c++;
      }
      NSRect runRect = NSMakeRect(kPadX + (CGFloat)runStart * cw, rowY,
                                  (CGFloat)(c - runStart) * cw, ch);
      if (sel) {
        CGContextSetFillColorWithColor(ctx, selBg);
        CGContextFillRect(ctx, runRect);
      } else {
        CGFloat comp[4] = {r0 / 255.0, g0 / 255.0, b0 / 255.0, 1.0};
        CGColorRef bgColor = CGColorCreate(cs, comp);
        CGContextSetFillColorWithColor(ctx, bgColor);
        CGContextFillRect(ctx, runRect);
        CFRelease(bgColor);
      }
    }
  }
  CFRelease(selBg);

  // Solid block cursor background (drawn BEFORE glyph pass so glyph renders inverted on top).
  if (showCursor && cursorSolidBlock && curY >= firstRow && curY <= lastRow) {
    CGFloat comp[4] = {snap->cursor.color[0] / 255.0,
                       snap->cursor.color[1] / 255.0,
                       snap->cursor.color[2] / 255.0, 1.0};
    CGColorRef curBg = CGColorCreate(cs, comp);
    CGContextSetFillColorWithColor(ctx, curBg);
    CGContextFillRect(ctx, NSMakeRect(kPadX + (CGFloat)curX * cw,
                                      kPadY + (CGFloat)curY * ch,
                                      cw * (CGFloat)curCells, ch));
    CFRelease(curBg);
  }

  // Pass 2: Glyphs, procedural Box/Block/Braille, underlines, strikethrough, overline.
  for (int r = firstRow; r <= lastRow; r++) {
    CGFloat rowY = kPadY + (CGFloat)r * ch;
    const CbRenderCell *rowCells = &snap->cells[(size_t)r * cols];
    for (uint16_t c = 0; c < cols; c++) {
      const CbRenderCell *cell = &rowCells[c];
      if (cell->wide == CB_WIDE_SPACER_TAIL ||
          cell->wide == CB_WIDE_SPACER_HEAD) {
        continue;
      }
      int span = (cell->wide == CB_WIDE_WIDE && c + 1 < cols) ? 2 : 1;
      CGFloat cellX = kPadX + (CGFloat)c * cw;
      CGFloat cellW = cw * (CGFloat)span;
      uint16_t flags = cell->flags;

      BOOL underSolidCursor =
          cursorSolidBlock && (r == (int)curY) &&
          (c >= curX && c < curX + (uint16_t)curCells);
      uint8_t fgR = underSolidCursor ? snap->cursor.text_color[0] : cell->fg[0];
      uint8_t fgG = underSolidCursor ? snap->cursor.text_color[1] : cell->fg[1];
      uint8_t fgB = underSolidCursor ? snap->cursor.text_color[2] : cell->fg[2];
      CGFloat alpha = (flags & CB_STYLE_FAINT) ? 0.58 : 1.0;

      CGFloat fgComp[4] = {fgR / 255.0, fgG / 255.0, fgB / 255.0, alpha};
      CGColorRef fgColor = CGColorCreate(cs, fgComp);

      uint32_t cp = cell->cp;
      if (cp > 0x20 && !(flags & CB_STYLE_INVISIBLE)) {
        NSRect cellRect = NSMakeRect(cellX, rowY, cellW, ch);
        if (!cb_draw_box_or_block(ctx, cp, cellRect, fgColor, _backingScale)) {
          UniChar localBuf[2];
          const UniChar *chars = NULL;
          CFIndex charLen = 0;
          if (cell->utf16_len > 0 && snap->utf16) {
            chars = &snap->utf16[cell->utf16_offset];
            charLen = (CFIndex)cell->utf16_len;
          } else if (cp <= 0xFFFF) {
            localBuf[0] = (UniChar)cp;
            chars = localBuf;
            charLen = 1;
          } else {
            uint32_t u = cp - 0x10000;
            localBuf[0] = (UniChar)(0xD800 + (u >> 10));
            localBuf[1] = (UniChar)(0xDC00 + (u & 0x3FF));
            chars = localBuf;
            charLen = 2;
          }

          CTFontRef font = [self resolveFontForUTF16:chars
                                              length:charLen
                                           codepoint:cp
                                               flags:flags];
          CGGlyph singleGlyph = 0;
          if (charLen == 1 &&
              CTFontGetGlyphsForCharacters(font, chars, &singleGlyph, 1) &&
              singleGlyph != 0) {
            CGContextSaveGState(ctx);
            CGContextTranslateCTM(ctx, cellX, rowY + _baselineOffset);
            CGContextScaleCTM(ctx, 1.0, -1.0);
            CGContextSetTextMatrix(ctx, CGAffineTransformIdentity);
            CGPoint pos = CGPointZero;
            CGContextSetFillColorWithColor(ctx, fgColor);
            CTFontDrawGlyphs(font, &singleGlyph, &pos, 1, ctx);
            CGContextRestoreGState(ctx);
          } else if (charLen > 0) {
            CFStringRef s = CFStringCreateWithCharactersNoCopy(
                kCFAllocatorDefault, chars, charLen, kCFAllocatorNull);
            if (s) {
              NSColor *nsFg = [NSColor colorWithDeviceRed:fgComp[0]
                                                    green:fgComp[1]
                                                     blue:fgComp[2]
                                                    alpha:fgComp[3]];
              NSDictionary *attrs = @{
                (__bridge id)kCTFontAttributeName : (__bridge id)font,
                (__bridge id)kCTForegroundColorAttributeName : nsFg,
              };
              NSAttributedString *attr =
                  [[NSAttributedString alloc] initWithString:(__bridge NSString *)s
                                                  attributes:attrs];
              CTLineRef line =
                  CTLineCreateWithAttributedString((__bridge CFAttributedStringRef)attr);
              if (line) {
                CGContextSaveGState(ctx);
                CGContextTranslateCTM(ctx, cellX, rowY + _baselineOffset);
                CGContextScaleCTM(ctx, 1.0, -1.0);
                CGContextSetTextMatrix(ctx, CGAffineTransformIdentity);
                CGContextSetTextPosition(ctx, 0.0, 0.0);
                CTLineDraw(line, ctx);
                CGContextRestoreGState(ctx);
                CFRelease(line);
              }
              CFRelease(s);
            }
          }
        }
      }

      int ulStyle = (flags & CB_STYLE_UNDERLINE_MASK) >> CB_STYLE_UNDERLINE_SHIFT;
      BOOL hoveredLink = _hoverLinkValid && ((uint16_t)r == _hoverLinkRow) &&
                         (c >= _hoverLinkStartCol && c <= _hoverLinkEndCol);
      if (hoveredLink && ulStyle == CB_UNDERLINE_NONE) {
        ulStyle = CB_UNDERLINE_SINGLE;
      }
      if (ulStyle != CB_UNDERLINE_NONE) {
        CGColorRef ulColor = fgColor;
        if (cell->has_ul_color && !underSolidCursor) {
          CGFloat ulComp[4] = {cell->ul_color[0] / 255.0,
                               cell->ul_color[1] / 255.0,
                               cell->ul_color[2] / 255.0, alpha};
          ulColor = CGColorCreate(cs, ulComp);
        } else {
          CFRetain(ulColor);
        }
        [self drawUnderlineInContext:ctx
                               style:ulStyle
                                   x:cellX
                             rowTopY:rowY
                               width:cellW
                               color:ulColor];
        CFRelease(ulColor);
      }

      if (flags & CB_STYLE_STRIKETHROUGH) {
        CGFloat thick = fmax(1.0 / _backingScale, _underlineThickness);
        CGContextSetFillColorWithColor(ctx, fgColor);
        CGContextFillRect(
            ctx, CGRectMake(cellX, rowY + round(ch * 0.52), cellW, thick));
      }
      if (flags & CB_STYLE_OVERLINE) {
        CGFloat thick = fmax(1.0 / _backingScale, _underlineThickness);
        CGContextSetFillColorWithColor(ctx, fgColor);
        CGContextFillRect(ctx, CGRectMake(cellX, rowY, cellW, thick));
      }
      CFRelease(fgColor);
    }
  }

  // Pass 3: Non-solid cursor styles (hollow block when unfocused or requested, bar, underline).
  if (showCursor && !cursorSolidBlock && curY < rows && curX < cols &&
      ![self hasMarkedText]) {
    CGFloat cx = kPadX + (CGFloat)curX * cw;
    CGFloat cy = kPadY + (CGFloat)curY * ch;
    CGFloat width = cw * (CGFloat)curCells;
    CGFloat comp[4] = {snap->cursor.color[0] / 255.0,
                       snap->cursor.color[1] / 255.0,
                       snap->cursor.color[2] / 255.0, 1.0};
    CGColorRef curColor = CGColorCreate(cs, comp);
    CGContextSetFillColorWithColor(ctx, curColor);
    CGContextSetStrokeColorWithColor(ctx, curColor);

    uint8_t style = _focusedState ? snap->cursor.style : CB_CURSOR_BLOCK_HOLLOW;
    if (style == CB_CURSOR_BAR) {
      CGFloat barW = fmax(2.0, round(cw * 0.22));
      CGContextFillRect(ctx, CGRectMake(cx, cy, barW, ch));
    } else if (style == CB_CURSOR_UNDERLINE) {
      CGFloat ulH = fmax(2.0, round(ch * 0.14));
      CGContextFillRect(ctx, CGRectMake(cx, cy + ch - ulH, width, ulH));
    } else {
      CGFloat lw = fmax(1.0, 1.5);
      CGContextSetLineWidth(ctx, lw);
      CGContextStrokeRect(
          ctx, CGRectInset(CGRectMake(cx, cy, width, ch), lw * 0.5, lw * 0.5));
    }
    CFRelease(curColor);
  }

  // Pass 4: IME preedit (markedText) overlay at cursor position.
  if ([self hasMarkedText] && self.markedTextStr.length > 0 && curY < rows) {
    CGFloat mx = kPadX + (CGFloat)curX * cw;
    CGFloat my = kPadY + (CGFloat)curY * ch;
    NSString *marked = self.markedTextStr;
    NSUInteger cellSpan = 0;
    for (NSUInteger idx = 0; idx < marked.length; idx++) {
      unichar u = [marked characterAtIndex:idx];
      uint32_t scalar = u;
      if (CFStringIsSurrogateHighCharacter(u) && idx + 1 < marked.length) {
        unichar u2 = [marked characterAtIndex:idx + 1];
        if (CFStringIsSurrogateLowCharacter(u2)) {
          scalar = CFStringGetLongCharacterForSurrogatePair(u, u2);
          idx++;
        }
      }
      cellSpan += (NSUInteger)cb_char_cells(scalar);
    }
    CGFloat mw =
        fmin(fmax(cw, (CGFloat)cellSpan * cw), self.bounds.size.width - mx - kPadX);
    CGFloat bgPreComp[4] = {bgR / 255.0, bgG / 255.0, bgB / 255.0, 0.96};
    CGColorRef preBg = CGColorCreate(cs, bgPreComp);
    CGContextSetFillColorWithColor(ctx, preBg);
    CGContextFillRect(ctx, CGRectMake(mx, my, mw, ch));
    CFRelease(preBg);

    NSColor *fgNs = [NSColor colorWithDeviceRed:snap->foreground[0] / 255.0
                                          green:snap->foreground[1] / 255.0
                                           blue:snap->foreground[2] / 255.0
                                          alpha:1.0];
    NSDictionary *attrs = @{
      (__bridge id)kCTFontAttributeName : (__bridge id)_fontRegular,
      (__bridge id)kCTForegroundColorAttributeName : fgNs,
    };
    NSAttributedString *attr =
        [[NSAttributedString alloc] initWithString:marked attributes:attrs];
    CTLineRef line =
        CTLineCreateWithAttributedString((__bridge CFAttributedStringRef)attr);
    if (line) {
      CGContextSaveGState(ctx);
      CGContextTranslateCTM(ctx, mx, my + _baselineOffset);
      CGContextScaleCTM(ctx, 1.0, -1.0);
      CGContextSetTextMatrix(ctx, CGAffineTransformIdentity);
      CGContextSetTextPosition(ctx, 0.0, 0.0);
      CTLineDraw(line, ctx);
      CGContextRestoreGState(ctx);
      CFRelease(line);
    }
    CGFloat ulComp[4] = {snap->cursor.color[0] / 255.0,
                         snap->cursor.color[1] / 255.0,
                         snap->cursor.color[2] / 255.0, 0.9};
    CGColorRef preUl = CGColorCreate(cs, ulComp);
    [self drawUnderlineInContext:ctx
                           style:CB_UNDERLINE_SINGLE
                               x:mx
                         rowTopY:my
                           width:mw
                           color:preUl];
    CFRelease(preUl);
  }

  // Pass 5: Scrollbar thumb overlay when scrolled up or actively scrolling.
  NSTimeInterval now = [NSProcessInfo processInfo].systemUptime;
  BOOL scrolledBack =
      (snap->scrollbar.total > snap->scrollbar.len) &&
      (snap->scrollbar.offset + snap->scrollbar.len < snap->scrollbar.total);
  if (snap->scrollbar.total > snap->scrollbar.len &&
      (scrolledBack || now < _scrollbarVisibleUntil + 0.2)) {
    CGFloat alpha = scrolledBack
                        ? 0.42
                        : (CGFloat)fmax(0.0, fmin(0.42, (_scrollbarVisibleUntil + 0.2 - now) * 2.1));
    if (alpha > 0.01) {
      CGFloat trackTop = 4.0;
      CGFloat trackH = fmax(24.0, self.bounds.size.height - 8.0);
      double total = (double)snap->scrollbar.total;
      double len = (double)snap->scrollbar.len;
      double off = (double)snap->scrollbar.offset;
      CGFloat thumbH = fmax(22.0, trackH * (CGFloat)(len / total));
      CGFloat maxTravel = fmax(0.0, trackH - thumbH);
      double ratio = (total > len) ? fmin(1.0, fmax(0.0, off / (total - len))) : 1.0;
      CGFloat thumbY = trackTop + maxTravel * (CGFloat)ratio;
      CGFloat thumbX = self.bounds.size.width - 6.0;
      NSBezierPath *pill =
          [NSBezierPath bezierPathWithRoundedRect:NSMakeRect(thumbX, thumbY, 3.5, thumbH)
                                          xRadius:1.75
                                          yRadius:1.75];
      [[NSColor colorWithDeviceRed:snap->foreground[0] / 255.0
                             green:snap->foreground[1] / 255.0
                              blue:snap->foreground[2] / 255.0
                             alpha:alpha] setFill];
      [pill fill];
    }
  }

  // Pass 6: Non-intrusive top-right status badge when connecting/disconnected/exited.
  if (snap->status_ptr && snap->status_len > 0) {
    NSString *status = [[NSString alloc] initWithBytes:snap->status_ptr
                                                length:snap->status_len
                                              encoding:NSUTF8StringEncoding];
    if (status.length > 0) {
      NSFont *badgeFont = [NSFont systemFontOfSize:11.0 weight:NSFontWeightMedium];
      NSColor *badgeFg = [NSColor colorWithDeviceRed:snap->foreground[0] / 255.0
                                               green:snap->foreground[1] / 255.0
                                                blue:snap->foreground[2] / 255.0
                                               alpha:0.86];
      NSDictionary *sAttrs = @{
        NSFontAttributeName : badgeFont,
        NSForegroundColorAttributeName : badgeFg,
      };
      NSSize tSize = [status sizeWithAttributes:sAttrs];
      CGFloat padH = 8.0, padV = 4.0;
      CGFloat bw = ceil(tSize.width) + padH * 2.0;
      CGFloat bh = ceil(tSize.height) + padV * 2.0;
      NSRect bRect =
          NSMakeRect(fmax(8.0, self.bounds.size.width - bw - 10.0), 8.0, bw, bh);
      NSBezierPath *badgePath = [NSBezierPath bezierPathWithRoundedRect:bRect
                                                                xRadius:6.0
                                                                yRadius:6.0];
      [[NSColor colorWithDeviceRed:bgR / 255.0
                             green:bgG / 255.0
                              blue:bgB / 255.0
                             alpha:0.88] setFill];
      [badgePath fill];
      [[badgeFg colorWithAlphaComponent:0.22] setStroke];
      [badgePath setLineWidth:1.0];
      [badgePath stroke];
      [status drawAtPoint:NSMakePoint(bRect.origin.x + padH, bRect.origin.y + padV)
           withAttributes:sAttrs];
    }
  }

  CGColorSpaceRelease(cs);
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

- (IBAction)paste:(id)sender {
  (void)sender;
  if (!_host || !_cb.paste)
    return;
  NSString *text =
      [[NSPasteboard generalPasteboard] stringForType:NSPasteboardTypeString];
  if (!text || text.length == 0)
    return;
  NSData *utf8 = [text dataUsingEncoding:NSUTF8StringEncoding];
  if (!utf8 || utf8.length == 0)
    return;
  _cb.paste(_host, (const uint8_t *)utf8.bytes, utf8.length);
  [self pollSnapshotForceFull:NO];
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
  _cb.clear_screen(_host);
  [self pollSnapshotForceFull:NO];
}

- (BOOL)validateMenuItem:(NSMenuItem *)menuItem {
  SEL action = menuItem.action;
  if (action == @selector(copy:)) {
    return _host && _cb.has_selection && _cb.has_selection(_host);
  }
  if (action == @selector(paste:)) {
    NSString *s = [[NSPasteboard generalPasteboard]
        stringForType:NSPasteboardTypeString];
    return s.length > 0;
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
        _cb.key(_host, kev);
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
      updatedMarked) {
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
    NSPoint localPt = [self convertPoint:winPt fromView:nil];
    [self refreshHoverStateAtPoint:localPt modifierFlags:flags];
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
      [self pollSnapshotForceFull:NO];
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
    self.markedRangeVal = NSMakeRange(NSNotFound, 0);
    self.selectedRangeVal = NSMakeRange(0, 0);
  }
  [self setNeedsDisplay:YES];
}

- (void)unmarkText {
  BOOL had = [self hasMarkedText];
  if (had) {
    self.didUpdateMarkedText = YES;
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
  NSRange clamped =
      NSIntersectionRange(range, NSMakeRange(0, self.markedTextStr.length));
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
  if (actualRange) {
    if ([self hasMarkedText]) {
      *actualRange =
          NSIntersectionRange(range, NSMakeRange(0, self.markedTextStr.length));
    } else {
      *actualRange = NSMakeRange(0, 0);
    }
  }
  CGFloat cw = _cellWidth > 0.0 ? _cellWidth : 8.0;
  CGFloat ch = _cellHeight > 0.0 ? _cellHeight : 18.0;
  CGFloat cx = kPadX;
  CGFloat cy = kPadY;
  if (_snap && _snap->cols > 0 && _snap->rows > 0) {
    uint16_t curX = _snap->cursor.x < _snap->cols ? _snap->cursor.x
                                                  : (_snap->cols - 1);
    uint16_t curY = _snap->cursor.y < _snap->rows ? _snap->cursor.y
                                                  : (_snap->rows - 1);
    cx = kPadX + (CGFloat)curX * cw;
    cy = kPadY + (CGFloat)curY * ch;
  }
  if ([self hasMarkedText] && range.location != NSNotFound &&
      range.location > 0 && range.location <= self.markedTextStr.length) {
    NSUInteger colsOffset = 0;
    for (NSUInteger i = 0; i < range.location; i++) {
      colsOffset += (NSUInteger)cb_char_cells([self.markedTextStr characterAtIndex:i]);
    }
    cx += (CGFloat)colsOffset * cw;
  }
  NSRect localRect = NSMakeRect(cx, cy, cw, ch);
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

- (void)refreshHoverStateAtPoint:(NSPoint)pt
                   modifierFlags:(NSEventModifierFlags)flags {
  if (!_host || !NSPointInRect(pt, self.bounds)) {
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

- (void)cursorUpdate:(NSEvent *)event {
  NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
  [self refreshHoverStateAtPoint:pt modifierFlags:event.modifierFlags];
}

- (void)mouseEntered:(NSEvent *)event {
  NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
  [self refreshHoverStateAtPoint:pt modifierFlags:event.modifierFlags];
}

- (void)mouseExited:(NSEvent *)event {
  (void)event;
  [self clearHoveredLink];
}

- (void)mouseMoved:(NSEvent *)event {
  if (!_host)
    return;
  NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
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

  CGFloat ch = _cellHeight > 1.0 ? _cellHeight : 18.0;
  CGFloat cw = _cellWidth > 1.0 ? _cellWidth : 8.0;
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
  [term pollSnapshotForceFull:YES];
  [term setNeedsDisplay:YES];
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

char *cb_terminal_list_fonts_json(void) {
  @autoreleasepool {
    cb_register_user_font_subdirs_once();
    NSFontManager *fm = [NSFontManager sharedFontManager];
    NSMutableSet<NSString *> *seen = [NSMutableSet set];
    NSMutableArray<NSString *> *monospaced = [NSMutableArray array];
    NSMutableArray<NSString *> *others = [NSMutableArray array];

    for (NSString *family in fm.availableFontFamilies) {
      if (!family || family.length == 0 || [family hasPrefix:@"."])
        continue;
      if ([seen containsObject:family])
        continue;
      NSFont *font = [fm fontWithFamily:family traits:0 weight:5 size:13.0];
      if (!font)
        font = [NSFont fontWithName:family size:13.0];
      if (!font)
        continue;
      [seen addObject:family];
      if (cb_is_ascii_monospaced((__bridge CTFontRef)font)) {
        [monospaced addObject:family];
      } else {
        [others addObject:family];
      }
    }

    [monospaced sortUsingSelector:@selector(localizedCaseInsensitiveCompare:)];
    [others sortUsingSelector:@selector(localizedCaseInsensitiveCompare:)];

    NSDictionary *payload =
        @{@"monospaced" : monospaced, @"others" : others};
    NSData *data = [NSJSONSerialization dataWithJSONObject:payload
                                                   options:0
                                                     error:nil];
    if (!data)
      return NULL;
    char *buf = (char *)malloc(data.length + 1);
    if (!buf)
      return NULL;
    memcpy(buf, data.bytes, data.length);
    buf[data.length] = '\0';
    return buf;
  }
}

void cb_terminal_free_cstring(char *ptr) {
  if (ptr) {
    free(ptr);
  }
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
