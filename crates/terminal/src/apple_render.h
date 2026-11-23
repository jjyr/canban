// Pure CoreGraphics + CoreText terminal renderer for Apple platforms (macOS / iOS).
// Must NOT import AppKit or UIKit.
#ifndef CANBAN_APPLE_RENDER_H
#define CANBAN_APPLE_RENDER_H

#import <CoreGraphics/CoreGraphics.h>
#import <CoreText/CoreText.h>
#import <Foundation/Foundation.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

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
  uint32_t image_id;
  uint64_t image_generation;
  uint8_t layer;
  int32_t z;
  int32_t viewport_col;
  int32_t viewport_row;
  uint32_t grid_cols;
  uint32_t grid_rows;
  uint32_t cell_offset_x;
  uint32_t cell_offset_y;
  uint32_t pixel_width;
  uint32_t pixel_height;
  uint32_t source_x;
  uint32_t source_y;
  uint32_t source_width;
  uint32_t source_height;
} CbRenderImagePlacement;

typedef struct {
  const uint8_t *rgba_ptr;
  size_t rgba_len;
  uint32_t width;
  uint32_t height;
  uint64_t generation;
} CbRenderImageData;

typedef struct {
  const CbRenderCell *cells;
  const uint16_t *utf16;
  const uint8_t *dirty_rows;
  const CbRenderImagePlacement *image_placements;
  size_t image_placements_len;
  uint64_t image_placements_generation;
  uint32_t next_animation_ms;
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
  uint32_t cell_width_milli;
  uint32_t cell_height_milli;
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
  const uint8_t *ptr;
  size_t len;
} CbByteSlice;

typedef struct {
  const CbSnapshot *(*snapshot)(void *, CbGeometry, bool);
  bool (*key)(void *, CbKeyEvent);
  void (*text)(void *, const uint8_t *, size_t);
  void (*paste)(void *, const uint8_t *, size_t);
  bool (*paste_paths)(void *, const CbByteSlice *, size_t);
  bool (*can_accept_input)(void *);
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
  uint16_t (*measure_text)(void *, const uint8_t *, size_t);
  bool (*resolve_image)(void *, uint32_t, CbRenderImageData *);
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

enum {
  CB_IMAGE_LAYER_BELOW_BG = 0,
  CB_IMAGE_LAYER_BELOW_TEXT = 1,
  CB_IMAGE_LAYER_ABOVE_TEXT = 2,
};

static const uint32_t kKittyUnicodePlaceholder = 0x10EEEE;
static const CGFloat kPadX = 8.0;
static const CGFloat kPadY = 6.0;

typedef struct {
  void *host;
  bool (*resolve_image)(void *, uint32_t, CbRenderImageData *);
  uint16_t (*measure_text)(void *, const uint8_t *, size_t);
  uint8_t theme_bg[3];
  bool focused;
  bool cursor_blink_visible;
  bool optimistic_cursor;
  uint16_t cur_x;
  uint16_t cur_y;
  bool has_marked_text;
  __unsafe_unretained NSString *marked_text;
  NSRange selected_range;
  bool hover_link_valid;
  uint16_t hover_link_row;
  uint16_t hover_link_start_col;
  uint16_t hover_link_end_col;
  double now;
  double scrollbar_visible_until;
} CbRenderDrawParams;

// Pure CoreGraphics / CoreText helper functions.
int cb_char_cells(uint32_t cp);
NSUInteger cb_clamp_utf16_boundary(NSString *s, NSUInteger idx);
bool cb_draw_box_or_block(CGContextRef ctx, uint32_t cp, CGRect cell,
                          CGColorRef color, CGFloat scale);
void cb_register_user_font_subdirs_once(void);
bool cb_is_ascii_monospaced(CTFontRef font);
CTFontRef cb_create_styled_font(NSString *family, CGFloat size,
                                CTFontSymbolicTraits traits);
bool cb_font_is_last_resort(CTFontRef font);
bool cb_font_covers_utf16(CTFontRef font, const UniChar *chars, CFIndex len,
                          CGGlyph *outFirstGlyph);
bool cb_is_nerd_or_pua_codepoint(uint32_t cp);
bool cb_is_powerline_divider(uint32_t cp);
NSArray<NSString *> *cb_nerd_fallback_families(void);
char *cb_terminal_list_fonts_json(void);
void cb_terminal_free_cstring(char *ptr);

@interface CbCachedKittyImage : NSObject {
@public
  uint32_t _imageId;
  uint64_t _generation;
  uint32_t _width;
  uint32_t _height;
  CGImageRef _cgImage;
}
@end

@interface CbAppleRenderer : NSObject {
@public
  CTFontRef _fontRegular;
  CTFontRef _fontBold;
  CTFontRef _fontItalic;
  CTFontRef _fontBoldItalic;
  CFMutableDictionaryRef _fallbackCache;
  NSMutableDictionary<NSNumber *, CbCachedKittyImage *> *_imageCache;
}
@property(nonatomic, copy) NSString *fontFamily;
@property(nonatomic) CGFloat fontSize;
@property(nonatomic) CGFloat cellWidth;
@property(nonatomic) CGFloat cellHeight;
@property(nonatomic) CGFloat baselineOffset;
@property(nonatomic) CGFloat underlinePosition;
@property(nonatomic) CGFloat underlineThickness;
@property(nonatomic) CGFloat backingScale;

- (void)rebuildFonts;
- (void)releaseFonts;
- (void)clearImageCache;
- (CTFontRef)fontForStyleFlags:(uint16_t)flags;
- (CTFontRef)resolveFontForUTF16:(const UniChar *)chars
                          length:(CFIndex)len
                       codepoint:(uint32_t)cp
                           flags:(uint16_t)flags;
- (void)drawUnderlineInContext:(CGContextRef)ctx
                         style:(int)ulStyle
                             x:(CGFloat)x
                       rowTopY:(CGFloat)rowY
                         width:(CGFloat)width
                         color:(CGColorRef)color;
- (CGImageRef)cachedKittyImageForId:(uint32_t)imageId
                         generation:(uint64_t)generation
                               host:(void *)host
                       resolveImage:(bool (*)(void *, uint32_t,
                                              CbRenderImageData *))resolveImage
                               snap:(const CbSnapshot *)snap
                           outWidth:(uint32_t *)outW
                          outHeight:(uint32_t *)outH;
- (void)drawKittyLayer:(uint8_t)layer
             inContext:(CGContextRef)ctx
             dirtyRect:(CGRect)dirtyRect
                  snap:(const CbSnapshot *)snap
                  host:(void *)host
          resolveImage:(bool (*)(void *, uint32_t,
                                 CbRenderImageData *))resolveImage;
- (void)drawSnapshot:(const CbSnapshot *)snap
           inContext:(CGContextRef)ctx
              bounds:(CGRect)bounds
           dirtyRect:(CGRect)dirtyRect
              params:(const CbRenderDrawParams *)params;
@end

#endif // CANBAN_APPLE_RENDER_H
