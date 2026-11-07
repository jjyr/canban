#include <ghostty/vt.h>

// Bindgen cannot evaluate upstream's macros containing GhosttyStyleFlags casts.
// Let Clang resolve their values rather than duplicating the ABI's bit layout.
static const GhosttyStyleFlags CANBAN_STYLE_BOLD = GHOSTTY_STYLE_FLAG_BOLD;
static const GhosttyStyleFlags CANBAN_STYLE_INVERSE = GHOSTTY_STYLE_FLAG_INVERSE;
static const GhosttyStyleFlags CANBAN_STYLE_INVISIBLE = GHOSTTY_STYLE_FLAG_INVISIBLE;
static const GhosttyStyleFlags CANBAN_STYLE_UNDERLINE = GHOSTTY_STYLE_UNDERLINE_MASK;
