// Pure CoreGraphics + CoreText terminal renderer for Apple platforms (macOS / iOS).
// ZERO AppKit / Cocoa imports.
#import "apple_render.h"
#include <math.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>

@implementation CbCachedKittyImage
- (void)dealloc {
  if (_cgImage) {
    CFRelease(_cgImage);
    _cgImage = NULL;
  }
}
@end

int cb_char_cells(uint32_t cp) {
  if (cp == 0)
    return 0;
  int w = wcwidth((wchar_t)cp);
  if (w <= 0)
    return 0;
  return w >= 2 ? 2 : 1;
}

NSUInteger cb_clamp_utf16_boundary(NSString *s, NSUInteger idx) {
  if (!s)
    return 0;
  NSUInteger len = s.length;
  if (idx >= len)
    return len;
  if (idx > 0 && CFStringIsSurrogateLowCharacter([s characterAtIndex:idx]) &&
      CFStringIsSurrogateHighCharacter([s characterAtIndex:idx - 1])) {
    return idx - 1;
  }
  return idx;
}

static NSUInteger cb_measure_cells(NSString *str, void *host,
                                   uint16_t (*measure_text)(void *,
                                                            const uint8_t *,
                                                            size_t)) {
  if (!str || str.length == 0)
    return 0;
  if (host && measure_text) {
    NSData *utf8 = [str dataUsingEncoding:NSUTF8StringEncoding];
    if (utf8 && utf8.length > 0) {
      return (NSUInteger)measure_text(host, (const uint8_t *)utf8.bytes,
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

bool cb_draw_box_or_block(CGContextRef ctx, uint32_t cp, CGRect cell,
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

void cb_register_user_font_subdirs_once(void) {
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

bool cb_is_ascii_monospaced(CTFontRef font) {
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

static CTFontRef cb_resolve_font_by_name_or_family(NSString *family,
                                                  CGFloat size) {
  if (!family || family.length == 0)
    return NULL;
  // 1. Try exact PostScript / font name match via CoreText descriptor.
  {
    NSDictionary *attrs = @{(__bridge id)kCTFontNameAttribute : family};
    CTFontDescriptorRef rawDesc =
        CTFontDescriptorCreateWithAttributes((__bridge CFDictionaryRef)attrs);
    if (rawDesc) {
      NSSet *mandatory =
          [NSSet setWithObject:(__bridge id)kCTFontNameAttribute];
      CTFontDescriptorRef matched =
          CTFontDescriptorCreateMatchingFontDescriptor(
              rawDesc, (__bridge CFSetRef)mandatory);
      CFRelease(rawDesc);
      if (matched) {
        CTFontRef ctFont = CTFontCreateWithFontDescriptor(matched, size, NULL);
        CFRelease(matched);
        if (ctFont)
          return ctFont;
      }
    }
  }
  // 2. Try family name match via CoreText descriptor.
  {
    NSDictionary *attrs = @{(__bridge id)kCTFontFamilyNameAttribute : family};
    CTFontDescriptorRef rawDesc =
        CTFontDescriptorCreateWithAttributes((__bridge CFDictionaryRef)attrs);
    if (rawDesc) {
      NSSet *mandatory =
          [NSSet setWithObject:(__bridge id)kCTFontFamilyNameAttribute];
      CTFontDescriptorRef matched =
          CTFontDescriptorCreateMatchingFontDescriptor(
              rawDesc, (__bridge CFSetRef)mandatory);
      CFRelease(rawDesc);
      if (matched) {
        CTFontRef ctFont = CTFontCreateWithFontDescriptor(matched, size, NULL);
        CFRelease(matched);
        if (ctFont)
          return ctFont;
      }
    }
  }
  return NULL;
}

static CTFontRef cb_create_system_mono_font(CGFloat size) {
  CTFontRef sfMono = cb_resolve_font_by_name_or_family(
      @".AppleSystemUIFontMonospaced-Regular", size);
  if (sfMono)
    return sfMono;
  CTFontRef uiFixed =
      CTFontCreateUIFontForLanguage(kCTFontUIFontUserFixedPitch, size, NULL);
  if (uiFixed)
    return uiFixed;
  return CTFontCreateWithName(CFSTR("Menlo"), size, NULL);
}

CTFontRef cb_create_styled_font(NSString *family, CGFloat size,
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
  CTFontRef ctBase = NULL;
  if (family && family.length > 0) {
    ctBase = cb_resolve_font_by_name_or_family(family, size);
    if (!ctBase) {
      cb_register_user_font_subdirs_once();
      ctBase = cb_resolve_font_by_name_or_family(family, size);
    }
  }
  if (!ctBase) {
    ctBase = cb_create_system_mono_font(size);
  }
  CTFontRef result = NULL;
  if (ctBase) {
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
    CFRelease(ctBase);
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

bool cb_font_is_last_resort(CTFontRef font) {
  if (!font)
    return true;
  CFStringRef ps = CTFontCopyPostScriptName(font);
  if (ps) {
    bool isLR = (CFStringCompare(ps, CFSTR("LastResort"), 0) == kCFCompareEqualTo) ||
                (CFStringCompare(ps, CFSTR(".LastResort"), 0) == kCFCompareEqualTo);
    CFRelease(ps);
    if (isLR)
      return true;
  }
  CFStringRef fam = CTFontCopyFamilyName(font);
  if (fam) {
    bool isLR = (CFStringCompare(fam, CFSTR(".LastResort"), 0) == kCFCompareEqualTo) ||
                (CFStringCompare(fam, CFSTR("LastResort"), 0) == kCFCompareEqualTo);
    CFRelease(fam);
    if (isLR)
      return true;
  }
  return false;
}

bool cb_font_covers_utf16(CTFontRef font, const UniChar *chars, CFIndex len,
                          CGGlyph *outFirstGlyph) {
  if (!font || !chars || len <= 0 || len > 8)
    return false;
  CGGlyph glyphs[8] = {0};
  if (!CTFontGetGlyphsForCharacters(font, chars, glyphs, len))
    return false;
  CFIndex i = 0;
  while (i < len) {
    if (glyphs[i] == 0)
      return false;
    if (CFStringIsSurrogateHighCharacter(chars[i]) && (i + 1) < len &&
        CFStringIsSurrogateLowCharacter(chars[i + 1])) {
      // CoreText maps UTF-16 surrogate pairs to [glyph, 0].
      i += 2;
    } else {
      i += 1;
    }
  }
  if (outFirstGlyph)
    *outFirstGlyph = glyphs[0];
  return true;
}

bool cb_is_nerd_or_pua_codepoint(uint32_t cp) {
  return (cp >= 0xE000 && cp <= 0xF8FF) ||
         (cp >= 0xF0000 && cp <= 0xFFFFD) ||
         (cp >= 0x100000 && cp <= 0x10FFFD) ||
         (cp >= 0x23FB && cp <= 0x23FE) ||
         cp == 0x2665 || cp == 0x26A1 || cp == 0x2B58;
}

bool cb_is_powerline_divider(uint32_t cp) {
  return (cp >= 0xE0B0 && cp <= 0xE0BF) || (cp >= 0xE0C0 && cp <= 0xE0D7);
}

NSArray<NSString *> *cb_nerd_fallback_families(void) {
  static NSArray<NSString *> *sFamilies = nil;
  static dispatch_once_t onceToken;
  dispatch_once(&onceToken, ^{
    cb_register_user_font_subdirs_once();
    NSMutableArray<NSString *> *ordered = [NSMutableArray array];
    NSMutableSet<NSString *> *seen = [NSMutableSet set];
    CFArrayRef familiesRef = CTFontManagerCopyAvailableFontFamilyNames();
    NSArray<NSString *> *avail =
        familiesRef ? CFBridgingRelease(familiesRef) : @[];
    NSSet<NSString *> *availSet = [NSSet setWithArray:avail];

    NSArray<NSString *> *preferred = @[
      @"Symbols Nerd Font Mono",
      @"Symbols Nerd Font",
      @"Nerd Fonts Symbols Only",
    ];
    for (NSString *cand in preferred) {
      if ([availSet containsObject:cand] && ![seen containsObject:cand]) {
        [ordered addObject:cand];
        [seen addObject:cand];
      }
    }

    NSMutableArray<NSString *> *others = [NSMutableArray array];
    for (NSString *fam in avail) {
      if (!fam || fam.length == 0 || [fam hasPrefix:@"."] ||
          [seen containsObject:fam]) {
        continue;
      }
      if ([fam rangeOfString:@"Nerd Font" options:NSCaseInsensitiveSearch]
                  .location != NSNotFound ||
          [fam rangeOfString:@"NerdFont" options:NSCaseInsensitiveSearch]
                  .location != NSNotFound ||
          [fam hasSuffix:@" NF"] || [fam hasSuffix:@" NFM"]) {
        [others addObject:fam];
        [seen addObject:fam];
      }
    }
    [others sortUsingSelector:@selector(localizedCaseInsensitiveCompare:)];
    [ordered addObjectsFromArray:others];
    sFamilies = [ordered copy];
  });
  return sFamilies;
}

static CTFontRef cb_resolve_nerd_fallback_retained(const UniChar *chars,
                                                   CFIndex len,
                                                   CGFloat size,
                                                   CTFontSymbolicTraits traits) {
  for (NSString *fam in cb_nerd_fallback_families()) {
    CTFontRef cand = cb_create_styled_font(fam, size, traits);
    if (!cand)
      continue;
    if (!cb_font_is_last_resort(cand) &&
        cb_font_covers_utf16(cand, chars, len, NULL)) {
      return cand;
    }
    CFRelease(cand);
  }
  return NULL;
}

static CTLineRef cb_create_ctline(CFStringRef str, CTFontRef font,
                                  CGColorRef color) {
  if (!str || !font || !color)
    return NULL;
  const void *keys[2] = {kCTFontAttributeName, kCTForegroundColorAttributeName};
  const void *vals[2] = {font, color};
  CFDictionaryRef attrs = CFDictionaryCreate(
      kCFAllocatorDefault, keys, vals, 2, &kCFTypeDictionaryKeyCallBacks,
      &kCFTypeDictionaryValueCallBacks);
  if (!attrs)
    return NULL;
  CFAttributedStringRef attrStr =
      CFAttributedStringCreate(kCFAllocatorDefault, str, attrs);
  CFRelease(attrs);
  if (!attrStr)
    return NULL;
  CTLineRef line = CTLineCreateWithAttributedString(attrStr);
  CFRelease(attrStr);
  return line;
}

@implementation CbAppleRenderer

- (instancetype)init {
  self = [super init];
  if (!self)
    return nil;
  _fontFamily = nil;
  _fontSize = 13.0;
  _backingScale = 2.0;
  _fallbackCache = CFDictionaryCreateMutable(
      kCFAllocatorDefault, 0, NULL, &kCFTypeDictionaryValueCallBacks);
  _imageCache = [NSMutableDictionary dictionary];
  [self rebuildFonts];
  return self;
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

- (void)clearImageCache {
  [_imageCache removeAllObjects];
}

- (void)dealloc {
  [self releaseFonts];
  if (_fallbackCache) {
    CFRelease(_fallbackCache);
    _fallbackCache = NULL;
  }
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
  if (cb_font_covers_utf16(primary, chars, len, NULL))
    return primary;

  uintptr_t key =
      ((uintptr_t)cp << 2) | (uintptr_t)(flags & (CB_STYLE_BOLD | CB_STYLE_ITALIC));
  if (_fallbackCache && key != 0) {
    CTFontRef cached =
        (CTFontRef)CFDictionaryGetValue(_fallbackCache, (const void *)key);
    if (cached)
      return cached;
  }

  CGFloat sz = fmax(9.0, fmin(32.0, _fontSize > 0.0 ? _fontSize : 13.0));
  CTFontSymbolicTraits traits = 0;
  if (flags & CB_STYLE_BOLD)
    traits |= kCTFontBoldTrait;
  if (flags & CB_STYLE_ITALIC)
    traits |= kCTFontItalicTrait;

  CTFontRef fb = NULL;
  if (cb_is_nerd_or_pua_codepoint(cp)) {
    fb = cb_resolve_nerd_fallback_retained(chars, len, sz, traits);
  }
  if (!fb) {
    CFStringRef str = CFStringCreateWithCharactersNoCopy(
        kCFAllocatorDefault, chars, len, kCFAllocatorNull);
    if (str) {
      fb = CTFontCreateForString(primary, str,
                                 CFRangeMake(0, CFStringGetLength(str)));
      CFRelease(str);
    }
    if (!fb || cb_font_is_last_resort(fb)) {
      CTFontRef nerdFb =
          cb_resolve_nerd_fallback_retained(chars, len, sz, traits);
      if (nerdFb) {
        if (fb)
          CFRelease(fb);
        fb = nerdFb;
      }
    }
  }
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

- (CGImageRef)cachedKittyImageForId:(uint32_t)imageId
                         generation:(uint64_t)generation
                               host:(void *)host
                       resolveImage:(bool (*)(void *, uint32_t,
                                              CbRenderImageData *))resolveImage
                               snap:(const CbSnapshot *)snap
                           outWidth:(uint32_t *)outW
                          outHeight:(uint32_t *)outH {
  if (!host || !resolveImage || imageId == 0)
    return NULL;
  NSNumber *key = @(imageId);
  CbCachedKittyImage *cached = _imageCache[key];
  if (cached && cached->_cgImage &&
      (generation == 0 || cached->_generation == generation)) {
    if (outW)
      *outW = cached->_width;
    if (outH)
      *outH = cached->_height;
    return cached->_cgImage;
  }

  CbRenderImageData imgData;
  memset(&imgData, 0, sizeof(imgData));
  if (!resolveImage(host, imageId, &imgData) || !imgData.rgba_ptr ||
      imgData.width == 0 || imgData.height == 0) {
    return NULL;
  }
  size_t expectedLen = (size_t)imgData.width * (size_t)imgData.height * 4;
  if (imgData.rgba_len < expectedLen)
    return NULL;

  if (cached && cached->_cgImage && cached->_generation == imgData.generation &&
      cached->_width == imgData.width && cached->_height == imgData.height) {
    if (outW)
      *outW = cached->_width;
    if (outH)
      *outH = cached->_height;
    return cached->_cgImage;
  }

  CFDataRef data = CFDataCreate(kCFAllocatorDefault, imgData.rgba_ptr,
                                (CFIndex)expectedLen);
  if (!data)
    return NULL;
  CGDataProviderRef provider = CGDataProviderCreateWithCFData(data);
  CFRelease(data);
  if (!provider)
    return NULL;

  CGColorSpaceRef cs = CGColorSpaceCreateDeviceRGB();
  CGImageRef cgImg = CGImageCreate(
      imgData.width, imgData.height, 8, 32, (size_t)imgData.width * 4, cs,
      (CGBitmapInfo)kCGImageAlphaLast | (CGBitmapInfo)kCGBitmapByteOrderDefault,
      provider, NULL, true, kCGRenderingIntentDefault);
  CGColorSpaceRelease(cs);
  CGDataProviderRelease(provider);
  if (!cgImg)
    return NULL;

  if (_imageCache.count >= 256 && snap && snap->image_placements &&
      snap->image_placements_len > 0) {
    NSMutableSet<NSNumber *> *active =
        [NSMutableSet setWithCapacity:snap->image_placements_len];
    for (size_t i = 0; i < snap->image_placements_len; i++) {
      [active addObject:@(snap->image_placements[i].image_id)];
    }
    NSArray<NSNumber *> *allKeys = [_imageCache allKeys];
    for (NSNumber *k in allKeys) {
      if (![active containsObject:k]) {
        [_imageCache removeObjectForKey:k];
      }
    }
    if (_imageCache.count >= 512) {
      [_imageCache removeAllObjects];
    }
  } else if (_imageCache.count >= 256) {
    [_imageCache removeAllObjects];
  }

  CbCachedKittyImage *entry = [[CbCachedKittyImage alloc] init];
  entry->_imageId = imageId;
  entry->_generation = imgData.generation;
  entry->_width = imgData.width;
  entry->_height = imgData.height;
  entry->_cgImage = cgImg;
  _imageCache[key] = entry;

  if (outW)
    *outW = imgData.width;
  if (outH)
    *outH = imgData.height;
  return cgImg;
}

- (void)drawKittyLayer:(uint8_t)layer
             inContext:(CGContextRef)ctx
             dirtyRect:(CGRect)dirtyRect
                  snap:(const CbSnapshot *)snap
                  host:(void *)host
          resolveImage:(bool (*)(void *, uint32_t,
                                 CbRenderImageData *))resolveImage {
  if (!snap || !snap->image_placements || snap->image_placements_len == 0 ||
      snap->cols == 0 || snap->rows == 0) {
    return;
  }

  CGFloat cw = _cellWidth > 0.0 ? _cellWidth : 8.0;
  CGFloat ch = _cellHeight > 0.0 ? _cellHeight : 18.0;
  CGRect viewportRect =
      CGRectMake(kPadX, kPadY, (CGFloat)snap->cols * cw, (CGFloat)snap->rows * ch);
  CGRect clipRect = CGRectIntersection(dirtyRect, viewportRect);
  if (CGRectIsEmpty(clipRect) || CGRectIsNull(clipRect))
    return;

  CGFloat geomCw = fmax(1.0, round(cw));
  CGFloat geomCh = fmax(1.0, round(ch));
  CGFloat scaleX = cw / geomCw;
  CGFloat scaleY = ch / geomCh;

  BOOL savedClip = NO;
  for (size_t i = 0; i < snap->image_placements_len; i++) {
    const CbRenderImagePlacement *p = &snap->image_placements[i];
    if (p->layer != layer || p->pixel_width == 0 || p->pixel_height == 0)
      continue;

    CGFloat destX = kPadX + (CGFloat)p->viewport_col * cw +
                    (CGFloat)p->cell_offset_x * scaleX;
    CGFloat destY = kPadY + (CGFloat)p->viewport_row * ch +
                    (CGFloat)p->cell_offset_y * scaleY;
    CGFloat destW = (CGFloat)p->pixel_width * scaleX;
    CGFloat destH = (CGFloat)p->pixel_height * scaleY;
    CGRect destRect = CGRectMake(destX, destY, destW, destH);
    if (!CGRectIntersectsRect(destRect, clipRect))
      continue;

    uint32_t imgW = 0, imgH = 0;
    CGImageRef fullImg = [self cachedKittyImageForId:p->image_id
                                          generation:p->image_generation
                                                host:host
                                        resolveImage:resolveImage
                                                snap:snap
                                            outWidth:&imgW
                                           outHeight:&imgH];
    if (!fullImg || imgW == 0 || imgH == 0)
      continue;

    if (!savedClip) {
      CGContextSaveGState(ctx);
      CGContextClipToRect(ctx, clipRect);
      CGContextSetInterpolationQuality(ctx, kCGInterpolationMedium);
      savedClip = YES;
    }

    uint32_t sx = p->source_x < imgW ? p->source_x : 0;
    uint32_t sy = p->source_y < imgH ? p->source_y : 0;
    uint32_t sw = (p->source_width > 0 && sx + p->source_width <= imgW)
                      ? p->source_width
                      : (imgW - sx);
    uint32_t sh = (p->source_height > 0 && sy + p->source_height <= imgH)
                      ? p->source_height
                      : (imgH - sy);
    if (sw == 0 || sh == 0)
      continue;

    CGImageRef drawImg = fullImg;
    BOOL releaseSubImg = NO;
    if (sx != 0 || sy != 0 || sw != imgW || sh != imgH) {
      drawImg = CGImageCreateWithImageInRect(
          fullImg, CGRectMake((CGFloat)sx, (CGFloat)sy, (CGFloat)sw, (CGFloat)sh));
      if (!drawImg)
        continue;
      releaseSubImg = YES;
    }

    CGContextSaveGState(ctx);
    CGContextTranslateCTM(ctx, destX, destY + destH);
    CGContextScaleCTM(ctx, 1.0, -1.0);
    CGContextDrawImage(ctx, CGRectMake(0.0, 0.0, destW, destH), drawImg);
    CGContextRestoreGState(ctx);

    if (releaseSubImg) {
      CFRelease(drawImg);
    }
  }

  if (savedClip) {
    CGContextRestoreGState(ctx);
  }
}

- (void)drawSnapshot:(const CbSnapshot *)snap
           inContext:(CGContextRef)ctx
              bounds:(CGRect)bounds
           dirtyRect:(CGRect)dirtyRect
              params:(const CbRenderDrawParams *)params {
  if (!ctx || !params)
    return;
  CGContextSaveGState(ctx);
  CGContextClipToRect(ctx, bounds);
  dirtyRect = CGRectIntersection(dirtyRect, bounds);
  if (CGRectIsEmpty(dirtyRect) || CGRectIsNull(dirtyRect)) {
    CGContextRestoreGState(ctx);
    return;
  }

  uint8_t bgR = snap ? snap->background[0] : params->theme_bg[0];
  uint8_t bgG = snap ? snap->background[1] : params->theme_bg[1];
  uint8_t bgB = snap ? snap->background[2] : params->theme_bg[2];
  CGColorSpaceRef cs = CGColorSpaceCreateDeviceRGB();

  CGFloat defaultBgComp[4] = {bgR / 255.0, bgG / 255.0, bgB / 255.0, 1.0};
  CGColorRef defaultBg = CGColorCreate(cs, defaultBgComp);
  CGContextSetFillColorWithColor(ctx, defaultBg);
  CGContextFillRect(ctx, dirtyRect);
  CFRelease(defaultBg);

  if (!snap || !snap->cells || snap->cols == 0 || snap->rows == 0) {
    CGColorSpaceRelease(cs);
    CGContextRestoreGState(ctx);
    return;
  }

  // Pass 0.5: Kitty Graphics placements below non-default cell backgrounds (z < INT32_MIN/2).
  [self drawKittyLayer:CB_IMAGE_LAYER_BELOW_BG
             inContext:ctx
             dirtyRect:dirtyRect
                  snap:snap
                  host:params->host
          resolveImage:params->resolve_image];

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
      ceil((CGRectGetMaxY(dirtyRect) - kPadY + 2.0) / ch));
  if (firstRow > lastRow) {
    firstRow = 0;
    lastRow = -1;
  }

  BOOL cursorSolidBlock = NO;
  uint16_t curX = params->cur_x;
  uint16_t curY = params->cur_y;
  BOOL optimisticCursor = params->optimistic_cursor;
  int curCells = 1;
  BOOL showCursor = snap->cursor.visible &&
                    (!params->focused || !snap->cursor.blinking ||
                     params->cursor_blink_visible);
  if (showCursor && curY < rows && curX < cols) {
    if (!optimisticCursor) {
      const CbRenderCell *cc = &snap->cells[(size_t)curY * cols + curX];
      if (cc->wide == CB_WIDE_WIDE && curX + 1 < cols) {
        curCells = 2;
      }
    }
    if (params->focused && snap->cursor.style == CB_CURSOR_BLOCK &&
        !params->has_marked_text) {
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
      CGRect runRect = CGRectMake(kPadX + (CGFloat)runStart * cw, rowY,
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
    CGContextFillRect(ctx, CGRectMake(kPadX + (CGFloat)curX * cw,
                                      kPadY + (CGFloat)curY * ch,
                                      cw * (CGFloat)curCells, ch));
    CFRelease(curBg);
  }

  // Pass 1.5: Kitty Graphics placements below text (INT32_MIN/2 <= z < 0, including U+10EEEE virtual placements).
  [self drawKittyLayer:CB_IMAGE_LAYER_BELOW_TEXT
             inContext:ctx
             dirtyRect:dirtyRect
                  snap:snap
                  host:params->host
          resolveImage:params->resolve_image];

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
      if (cp > 0x20 && cp != kKittyUnicodePlaceholder &&
          !(flags & CB_STYLE_INVISIBLE)) {
        CGRect cellRect = CGRectMake(cellX, rowY, cellW, ch);
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
          CTFontRef primary = [self fontForStyleFlags:flags];
          BOOL isSingleScalar =
              (charLen == 1) ||
              (charLen == 2 && CFStringIsSurrogateHighCharacter(chars[0]) &&
               CFStringIsSurrogateLowCharacter(chars[1]));
          BOOL isColorFont =
              (CTFontGetSymbolicTraits(font) & kCTFontColorGlyphsTrait) != 0;
          CGGlyph singleGlyph = 0;
          if (isSingleScalar && !isColorFont &&
              cb_font_covers_utf16(font, chars, charLen, &singleGlyph) &&
              singleGlyph != 0) {
            CGFloat scaleX = 1.0;
            CGFloat scaleY = 1.0;
            CGFloat dx = 0.0;
            CGFloat dy = 0.0;
            if (font != primary && cb_is_nerd_or_pua_codepoint(cp)) {
              CGRect bbox = CGRectZero;
              CTFontGetBoundingRectsForGlyphs(
                  font, kCTFontOrientationHorizontal, &singleGlyph, &bbox, 1);
              if (bbox.size.width > 0.05 && bbox.size.height > 0.05) {
                if (cb_is_powerline_divider(cp)) {
                  scaleY = ch / bbox.size.height;
                  scaleX = fmin(scaleY, cellW / bbox.size.width);
                  CGFloat scaledW = bbox.size.width * scaleX;
                  if (cp == 0xE0B2 || cp == 0xE0B3 || cp == 0xE0B6 ||
                      cp == 0xE0B7 || cp == 0xE0C2 || cp == 0xE0C3 ||
                      cp == 0xE0C6 || cp == 0xE0C7) {
                    dx = (cellW - scaledW) - bbox.origin.x * scaleX;
                  } else {
                    dx = -bbox.origin.x * scaleX;
                  }
                  CGFloat cellMidY = _baselineOffset - ch * 0.5;
                  CGFloat glyphMidY =
                      (bbox.origin.y + bbox.size.height * 0.5) * scaleY;
                  dy = cellMidY - glyphMidY;
                } else {
                  CGFloat availW = fmax(1.0, cellW - 0.5);
                  CGFloat availH = fmax(1.0, ch - 2.0);
                  CGFloat s = fmin(1.25, fmin(availW / bbox.size.width,
                                              availH / bbox.size.height));
                  scaleX = s;
                  scaleY = s;
                  CGFloat scaledW = bbox.size.width * scaleX;
                  dx = (cellW - scaledW) * 0.5 - bbox.origin.x * scaleX;
                  CGFloat primaryCapMid = CTFontGetCapHeight(primary) * 0.5;
                  CGFloat glyphMidY =
                      (bbox.origin.y + bbox.size.height * 0.5) * scaleY;
                  dy = primaryCapMid - glyphMidY;
                }
              }
            }
            CGContextSaveGState(ctx);
            CGContextTranslateCTM(ctx, cellX + dx,
                                  rowY + _baselineOffset - dy);
            CGContextScaleCTM(ctx, scaleX, -scaleY);
            CGContextSetTextMatrix(ctx, CGAffineTransformIdentity);
            CGPoint pos = CGPointZero;
            CGContextSetFillColorWithColor(ctx, fgColor);
            CTFontDrawGlyphs(font, &singleGlyph, &pos, 1, ctx);
            CGContextRestoreGState(ctx);
          } else if (charLen > 0) {
            CFStringRef s = CFStringCreateWithCharactersNoCopy(
                kCFAllocatorDefault, chars, charLen, kCFAllocatorNull);
            if (s) {
              CTLineRef line = cb_create_ctline(s, font, fgColor);
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
      BOOL hoveredLink = params->hover_link_valid &&
                         ((uint16_t)r == params->hover_link_row) &&
                         (c >= params->hover_link_start_col &&
                          c <= params->hover_link_end_col);
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

  // Pass 2.5: Kitty Graphics placements above text (z >= 0).
  [self drawKittyLayer:CB_IMAGE_LAYER_ABOVE_TEXT
             inContext:ctx
             dirtyRect:dirtyRect
                  snap:snap
                  host:params->host
          resolveImage:params->resolve_image];

  // Pass 3: Non-solid cursor styles (hollow block when unfocused or requested, bar, underline).
  if (showCursor && !cursorSolidBlock && curY < rows && curX < cols &&
      !params->has_marked_text) {
    CGFloat cx = kPadX + (CGFloat)curX * cw;
    CGFloat cy = kPadY + (CGFloat)curY * ch;
    CGFloat width = cw * (CGFloat)curCells;
    CGFloat comp[4] = {snap->cursor.color[0] / 255.0,
                       snap->cursor.color[1] / 255.0,
                       snap->cursor.color[2] / 255.0, 1.0};
    CGColorRef curColor = CGColorCreate(cs, comp);
    CGContextSetFillColorWithColor(ctx, curColor);
    CGContextSetStrokeColorWithColor(ctx, curColor);

    uint8_t style = params->focused ? snap->cursor.style : CB_CURSOR_BLOCK_HOLLOW;
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

  // Pass 4: IME preedit (markedText) overlay at cursor position with active caret.
  if (params->has_marked_text && params->marked_text &&
      params->marked_text.length > 0 && curY < rows) {
    uint16_t anchorCol = curX < cols ? curX : (cols - 1);
    CGFloat mx = kPadX + (CGFloat)anchorCol * cw;
    CGFloat my = kPadY + (CGFloat)curY * ch;
    NSString *marked = params->marked_text;
    NSUInteger cellSpan =
        cb_measure_cells(marked, params->host, params->measure_text);
    CGFloat preFgComp[4] = {snap->foreground[0] / 255.0,
                            snap->foreground[1] / 255.0,
                            snap->foreground[2] / 255.0, 1.0};
    CGColorRef preFg = CGColorCreate(cs, preFgComp);
    CTLineRef line =
        cb_create_ctline((__bridge CFStringRef)marked, _fontRegular, preFg);
    CFRelease(preFg);
    CGFloat lineTypoW =
        line ? ceil(CTLineGetTypographicBounds(line, NULL, NULL, NULL)) : 0.0;
    CGFloat maxAvailW = fmax(cw, bounds.size.width - mx - kPadX);
    CGFloat mw = fmin(fmax(cw, fmax((CGFloat)cellSpan * cw, lineTypoW)),
                      maxAvailW);

    CGFloat bgPreComp[4] = {bgR / 255.0, bgG / 255.0, bgB / 255.0, 0.96};
    CGColorRef preBg = CGColorCreate(cs, bgPreComp);
    CGContextSetFillColorWithColor(ctx, preBg);
    CGContextFillRect(ctx, CGRectMake(mx, my, mw, ch));
    CFRelease(preBg);

    if (line) {
      CGContextSaveGState(ctx);
      CGContextTranslateCTM(ctx, mx, my + _baselineOffset);
      CGContextScaleCTM(ctx, 1.0, -1.0);
      CGContextSetTextMatrix(ctx, CGAffineTransformIdentity);
      CGContextSetTextPosition(ctx, 0.0, 0.0);
      CTLineDraw(line, ctx);
      CGContextRestoreGState(ctx);
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

    NSRange selRange = params->selected_range;
    if (selRange.location != NSNotFound && selRange.length > 0 && line) {
      NSUInteger clauseStart = cb_clamp_utf16_boundary(marked, selRange.location);
      NSUInteger clauseEnd =
          cb_clamp_utf16_boundary(marked, NSMaxRange(selRange));
      CGFloat sx = CTLineGetOffsetForStringIndex(line, (CFIndex)clauseStart, NULL);
      CGFloat ex = CTLineGetOffsetForStringIndex(line, (CFIndex)clauseEnd, NULL);
      CGFloat clauseW = fmin(mw - sx, fmax(0.0, ex - sx));
      if (clauseW > 0.5) {
        [self drawUnderlineInContext:ctx
                               style:CB_UNDERLINE_DOUBLE
                                   x:mx + sx
                             rowTopY:my
                               width:clauseW
                               color:preUl];
      }
    }
    if (params->focused) {
      NSUInteger rawCaret = (selRange.location == NSNotFound)
                                ? marked.length
                                : MIN(marked.length, NSMaxRange(selRange));
      NSUInteger caretIdx = cb_clamp_utf16_boundary(marked, rawCaret);
      CGFloat caretOff = 0.0;
      if (line) {
        caretOff = CTLineGetOffsetForStringIndex(line, (CFIndex)caretIdx, NULL);
      } else if (caretIdx > 0) {
        caretOff = (CGFloat)cb_measure_cells(
                       [marked substringToIndex:caretIdx], params->host,
                       params->measure_text) *
                   cw;
      }
      CGFloat barW = fmax(2.0, round(cw * 0.22));
      CGFloat caretX = mx + fmax(0.0, fmin(mw, caretOff));
      if (caretX + barW > bounds.size.width - kPadX) {
        caretX = fmax(mx, bounds.size.width - kPadX - barW);
      }
      CGContextSetFillColorWithColor(ctx, preUl);
      CGContextFillRect(ctx, CGRectMake(caretX, my, barW, ch));
    }

    CFRelease(preUl);
    if (line) {
      CFRelease(line);
    }
  }

  // Pass 5: Scrollbar thumb overlay when scrolled up or actively scrolling.
  BOOL scrolledBack =
      (snap->scrollbar.total > snap->scrollbar.len) &&
      (snap->scrollbar.offset + snap->scrollbar.len < snap->scrollbar.total);
  if (snap->scrollbar.total > snap->scrollbar.len &&
      (scrolledBack || params->now < params->scrollbar_visible_until + 0.2)) {
    CGFloat alpha =
        scrolledBack
            ? 0.42
            : (CGFloat)fmax(
                  0.0, fmin(0.42, (params->scrollbar_visible_until + 0.2 -
                                   params->now) *
                                      2.1));
    if (alpha > 0.01) {
      CGFloat trackTop = 4.0;
      CGFloat trackH = fmax(24.0, bounds.size.height - 8.0);
      double total = (double)snap->scrollbar.total;
      double len = (double)snap->scrollbar.len;
      double off = (double)snap->scrollbar.offset;
      CGFloat thumbH = fmax(22.0, trackH * (CGFloat)(len / total));
      CGFloat maxTravel = fmax(0.0, trackH - thumbH);
      double ratio =
          (total > len) ? fmin(1.0, fmax(0.0, off / (total - len))) : 1.0;
      CGFloat thumbY = trackTop + maxTravel * (CGFloat)ratio;
      CGFloat thumbX = bounds.size.width - 6.0;
      CGPathRef pill = CGPathCreateWithRoundedRect(
          CGRectMake(thumbX, thumbY, 3.5, thumbH), 1.75, 1.75, NULL);
      CGFloat thumbComp[4] = {snap->foreground[0] / 255.0,
                              snap->foreground[1] / 255.0,
                              snap->foreground[2] / 255.0, alpha};
      CGColorRef thumbColor = CGColorCreate(cs, thumbComp);
      CGContextSaveGState(ctx);
      CGContextSetFillColorWithColor(ctx, thumbColor);
      CGContextAddPath(ctx, pill);
      CGContextFillPath(ctx);
      CGContextRestoreGState(ctx);
      CFRelease(thumbColor);
      CGPathRelease(pill);
    }
  }

  // Pass 6: Non-intrusive top-right status badge when connecting/disconnected/exited.
  if (snap->status_ptr && snap->status_len > 0) {
    NSString *status = [[NSString alloc] initWithBytes:snap->status_ptr
                                                length:snap->status_len
                                              encoding:NSUTF8StringEncoding];
    if (status.length > 0) {
      CTFontRef badgeFont =
          CTFontCreateUIFontForLanguage(kCTFontUIFontSystem, 11.0, NULL);
      if (badgeFont) {
        CGFloat fgBadgeComp[4] = {snap->foreground[0] / 255.0,
                                  snap->foreground[1] / 255.0,
                                  snap->foreground[2] / 255.0, 0.86};
        CGColorRef badgeFg = CGColorCreate(cs, fgBadgeComp);
        CTLineRef badgeLine = cb_create_ctline((__bridge CFStringRef)status,
                                               badgeFont, badgeFg);
        if (badgeLine) {
          CGFloat ascent = 0.0, descent = 0.0, leading = 0.0;
          CGFloat textW = (CGFloat)CTLineGetTypographicBounds(
              badgeLine, &ascent, &descent, &leading);
          CGFloat textH = ceil(ascent + descent);
          CGFloat padH = 8.0, padV = 4.0;
          CGFloat bw = ceil(textW) + padH * 2.0;
          CGFloat bh = textH + padV * 2.0;
          CGRect bRect = CGRectMake(fmax(8.0, bounds.size.width - bw - 10.0),
                                    8.0, bw, bh);
          CGPathRef badgePath =
              CGPathCreateWithRoundedRect(bRect, 6.0, 6.0, NULL);

          CGFloat bgBadgeComp[4] = {bgR / 255.0, bgG / 255.0, bgB / 255.0, 0.88};
          CGColorRef badgeBg = CGColorCreate(cs, bgBadgeComp);
          CGFloat borderComp[4] = {snap->foreground[0] / 255.0,
                                   snap->foreground[1] / 255.0,
                                   snap->foreground[2] / 255.0, 0.22};
          CGColorRef badgeBorder = CGColorCreate(cs, borderComp);

          CGContextSaveGState(ctx);
          CGContextSetFillColorWithColor(ctx, badgeBg);
          CGContextAddPath(ctx, badgePath);
          CGContextFillPath(ctx);
          CGContextSetStrokeColorWithColor(ctx, badgeBorder);
          CGContextSetLineWidth(ctx, 1.0);
          CGContextAddPath(ctx, badgePath);
          CGContextStrokePath(ctx);

          CGContextTranslateCTM(ctx, bRect.origin.x + padH,
                                bRect.origin.y + padV + ascent);
          CGContextScaleCTM(ctx, 1.0, -1.0);
          CGContextSetTextMatrix(ctx, CGAffineTransformIdentity);
          CGContextSetTextPosition(ctx, 0.0, 0.0);
          CTLineDraw(badgeLine, ctx);
          CGContextRestoreGState(ctx);

          CFRelease(badgeBg);
          CFRelease(badgeBorder);
          CGPathRelease(badgePath);
          CFRelease(badgeLine);
        }
        CFRelease(badgeFg);
        CFRelease(badgeFont);
      }
    }
  }

  CGColorSpaceRelease(cs);
  CGContextRestoreGState(ctx);
}

@end

char *cb_terminal_list_fonts_json(void) {
  @autoreleasepool {
    cb_register_user_font_subdirs_once();
    CFArrayRef families = CTFontManagerCopyAvailableFontFamilyNames();
    NSMutableSet<NSString *> *seen = [NSMutableSet set];
    NSMutableArray<NSString *> *monospaced = [NSMutableArray array];
    NSMutableArray<NSString *> *others = [NSMutableArray array];

    if (families) {
      CFIndex count = CFArrayGetCount(families);
      for (CFIndex i = 0; i < count; i++) {
        NSString *family =
            (__bridge NSString *)CFArrayGetValueAtIndex(families, i);
        if (!family || family.length == 0 || [family hasPrefix:@"."])
          continue;
        if ([seen containsObject:family])
          continue;

        NSDictionary *attrs =
            @{(__bridge id)kCTFontFamilyNameAttribute : family};
        CTFontDescriptorRef rawDesc =
            CTFontDescriptorCreateWithAttributes((__bridge CFDictionaryRef)attrs);
        CTFontRef font = NULL;
        if (rawDesc) {
          CTFontDescriptorRef matched =
              CTFontDescriptorCreateMatchingFontDescriptor(rawDesc, NULL);
          if (matched) {
            font = CTFontCreateWithFontDescriptor(matched, 13.0, NULL);
            CFRelease(matched);
          }
          CFRelease(rawDesc);
        }
        if (!font) {
          font = CTFontCreateWithName((__bridge CFStringRef)family, 13.0, NULL);
        }
        if (!font)
          continue;

        [seen addObject:family];
        if (cb_is_ascii_monospaced(font)) {
          [monospaced addObject:family];
        } else {
          [others addObject:family];
        }
        CFRelease(font);
      }
      CFRelease(families);
    }

    [monospaced sortUsingSelector:@selector(localizedCaseInsensitiveCompare:)];
    [others sortUsingSelector:@selector(localizedCaseInsensitiveCompare:)];
    (void)cb_nerd_fallback_families();

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
