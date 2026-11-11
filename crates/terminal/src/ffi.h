#include <ghostty/vt.h>

// Bindgen cannot evaluate upstream's macros containing GhosttyStyleFlags casts.
// Let Clang resolve their values rather than duplicating the ABI's bit layout.
static const GhosttyStyleFlags CANBAN_STYLE_BOLD = GHOSTTY_STYLE_FLAG_BOLD;
static const GhosttyStyleFlags CANBAN_STYLE_ITALIC = GHOSTTY_STYLE_FLAG_ITALIC;
static const GhosttyStyleFlags CANBAN_STYLE_FAINT = GHOSTTY_STYLE_FLAG_FAINT;
static const GhosttyStyleFlags CANBAN_STYLE_BLINK = GHOSTTY_STYLE_FLAG_BLINK;
static const GhosttyStyleFlags CANBAN_STYLE_INVERSE = GHOSTTY_STYLE_FLAG_INVERSE;
static const GhosttyStyleFlags CANBAN_STYLE_INVISIBLE = GHOSTTY_STYLE_FLAG_INVISIBLE;
static const GhosttyStyleFlags CANBAN_STYLE_STRIKETHROUGH = GHOSTTY_STYLE_FLAG_STRIKETHROUGH;
static const GhosttyStyleFlags CANBAN_STYLE_OVERLINE = GHOSTTY_STYLE_FLAG_OVERLINE;
static const GhosttyStyleFlags CANBAN_STYLE_UNDERLINE = GHOSTTY_STYLE_UNDERLINE_MASK;
static const GhosttyStyleFlags CANBAN_STYLE_UNDERLINE_SHIFT = GHOSTTY_STYLE_UNDERLINE_SHIFT;

