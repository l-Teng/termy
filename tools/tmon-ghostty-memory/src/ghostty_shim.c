#define GHOSTTY_STATIC 1

#include <ghostty/vt.h>
#include <libproc.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

enum {
  TMON_GHOSTTY_SHIM_OK = 0,
  TMON_GHOSTTY_SHIM_INVALID_ARGUMENT = 1,
  TMON_GHOSTTY_SHIM_GHOSTTY_ERROR = 2,
  TMON_GHOSTTY_SHIM_OUT_OF_MEMORY = 3,
  TMON_GHOSTTY_SHIM_PROC_ERROR = 4,
};

enum {
  TMON_GHOSTTY_COMPRESSION_COMPLETE = 0,
  TMON_GHOSTTY_COMPRESSION_UNSUPPORTED = 1,
  TMON_GHOSTTY_COMPRESSION_ERROR = 2,
};

typedef struct {
  uint64_t total;
  uint64_t offset;
  uint64_t len;
  uint16_t cursor_x;
  uint16_t cursor_y;
  uint8_t cursor_visible;
  uint8_t _padding[5];
} TmonGhosttyTerminalState;

typedef struct {
  uint64_t state;
} Fnv1a64;

static void fnv1a64_init(Fnv1a64* hash) {
  hash->state = UINT64_C(14695981039346656037);
}

static void fnv1a64_write_byte(Fnv1a64* hash, uint8_t byte) {
  hash->state ^= (uint64_t)byte;
  hash->state *= UINT64_C(1099511628211);
}

static void fnv1a64_write_u16(Fnv1a64* hash, uint16_t value) {
  fnv1a64_write_byte(hash, (uint8_t)(value & 0xffu));
  fnv1a64_write_byte(hash, (uint8_t)((value >> 8) & 0xffu));
}

static void fnv1a64_write_u32(Fnv1a64* hash, uint32_t value) {
  for (size_t i = 0; i < 4; i++) {
    fnv1a64_write_byte(hash, (uint8_t)((value >> (i * 8u)) & 0xffu));
  }
}

static void fnv1a64_write_u64(Fnv1a64* hash, uint64_t value) {
  for (size_t i = 0; i < 8; i++) {
    fnv1a64_write_byte(hash, (uint8_t)((value >> (i * 8u)) & 0xffu));
  }
}

static int ghostty_result_code(GhosttyResult result) {
  return result == GHOSTTY_OUT_OF_MEMORY ? TMON_GHOSTTY_SHIM_OUT_OF_MEMORY
                                         : TMON_GHOSTTY_SHIM_GHOSTTY_ERROR;
}

static size_t select_sample_rows(size_t total_rows, size_t out_rows[64]) {
  if (total_rows <= 64u) {
    for (size_t i = 0; i < total_rows; i++) {
      out_rows[i] = i;
    }
    return total_rows;
  }

  const size_t candidates[6] = {
      0u,
      1u,
      total_rows / 4u,
      total_rows / 2u,
      total_rows - 2u,
      total_rows - 1u,
  };
  size_t written = 0u;
  for (size_t i = 0; i < 6u; i++) {
    const size_t candidate = candidates[i];
    bool seen = false;
    for (size_t j = 0; j < written; j++) {
      if (out_rows[j] == candidate) {
        seen = true;
        break;
      }
    }
    if (!seen) {
      out_rows[written++] = candidate;
    }
  }
  return written;
}

static void fnv1a64_write_u8(Fnv1a64* hash, uint8_t value) {
  fnv1a64_write_byte(hash, value);
}

static int hash_style_color(Fnv1a64* hash, GhosttyStyleColor color) {
  fnv1a64_write_u8(hash, (uint8_t)color.tag);
  switch (color.tag) {
    case GHOSTTY_STYLE_COLOR_NONE:
      return TMON_GHOSTTY_SHIM_OK;
    case GHOSTTY_STYLE_COLOR_PALETTE:
      fnv1a64_write_u8(hash, color.value.palette);
      return TMON_GHOSTTY_SHIM_OK;
    case GHOSTTY_STYLE_COLOR_RGB:
      fnv1a64_write_u8(hash, color.value.rgb.r);
      fnv1a64_write_u8(hash, color.value.rgb.g);
      fnv1a64_write_u8(hash, color.value.rgb.b);
      return TMON_GHOSTTY_SHIM_OK;
    default:
      return TMON_GHOSTTY_SHIM_GHOSTTY_ERROR;
  }
}

static int hash_cell(Fnv1a64* hash, const GhosttyGridRef* ref) {
  GhosttyCell cell = 0;
  GhosttyStyle style = GHOSTTY_INIT_SIZED(GhosttyStyle);
  GhosttyCellWide wide = GHOSTTY_CELL_WIDE_NARROW;
  GhosttyResult result = ghostty_grid_ref_cell(ref, &cell);
  if (result != GHOSTTY_SUCCESS) {
    return ghostty_result_code(result);
  }
  result = ghostty_grid_ref_style(ref, &style);
  if (result != GHOSTTY_SUCCESS) {
    return ghostty_result_code(result);
  }
  result = ghostty_cell_get(cell, GHOSTTY_CELL_DATA_WIDE, &wide);
  if (result != GHOSTTY_SUCCESS) {
    return ghostty_result_code(result);
  }

  uint8_t role = 0u;
  switch (wide) {
    case GHOSTTY_CELL_WIDE_NARROW:
    case GHOSTTY_CELL_WIDE_WIDE:
      role = 0u;
      break;
    case GHOSTTY_CELL_WIDE_SPACER_TAIL:
      role = 1u;
      break;
    case GHOSTTY_CELL_WIDE_SPACER_HEAD:
      role = 2u;
      break;
    default:
      return TMON_GHOSTTY_SHIM_GHOSTTY_ERROR;
  }
  fnv1a64_write_u8(hash, role);

  uint32_t stack_graphemes[8];
  uint32_t* graphemes = stack_graphemes;
  size_t grapheme_len = 0u;
  result = ghostty_grid_ref_graphemes(ref, graphemes, 8u, &grapheme_len);
  if (result == GHOSTTY_OUT_OF_SPACE) {
    graphemes = (uint32_t*)malloc(grapheme_len * sizeof(uint32_t));
    if (graphemes == NULL) {
      return TMON_GHOSTTY_SHIM_OUT_OF_MEMORY;
    }
    result = ghostty_grid_ref_graphemes(ref, graphemes, grapheme_len, &grapheme_len);
  }
  if (result != GHOSTTY_SUCCESS) {
    if (graphemes != stack_graphemes) {
      free(graphemes);
    }
    return ghostty_result_code(result);
  }

  if (grapheme_len == 0u) {
    fnv1a64_write_u64(hash, 1u);
    fnv1a64_write_u32(hash, 0x20u);
  } else {
    fnv1a64_write_u64(hash, grapheme_len);
    for (size_t i = 0; i < grapheme_len; i++) {
      fnv1a64_write_u32(hash, graphemes[i]);
    }
  }

  if (graphemes != stack_graphemes) {
    free(graphemes);
  }

  int color_result = hash_style_color(hash, style.fg_color);
  if (color_result != TMON_GHOSTTY_SHIM_OK) {
    return color_result;
  }
  color_result = hash_style_color(hash, style.bg_color);
  if (color_result != TMON_GHOSTTY_SHIM_OK) {
    return color_result;
  }
  color_result = hash_style_color(hash, style.underline_color);
  if (color_result != TMON_GHOSTTY_SHIM_OK) {
    return color_result;
  }

  fnv1a64_write_u8(hash, style.bold ? 1u : 0u);
  fnv1a64_write_u8(hash, style.faint ? 1u : 0u);
  fnv1a64_write_u8(hash, style.italic ? 1u : 0u);
  fnv1a64_write_u8(hash, style.inverse ? 1u : 0u);
  fnv1a64_write_u8(hash, style.invisible ? 1u : 0u);
  fnv1a64_write_u8(hash, style.strikethrough ? 1u : 0u);
  fnv1a64_write_u32(hash, (uint32_t)style.underline);
  return TMON_GHOSTTY_SHIM_OK;
}

int tmon_ghostty_terminal_new(size_t scrollback_max_lines, void** out_terminal) {
  if (out_terminal == NULL) {
    return TMON_GHOSTTY_SHIM_INVALID_ARGUMENT;
  }
  *out_terminal = NULL;

  GhosttyTerminal terminal = NULL;
  GhosttyResult result = ghostty_terminal_new(NULL, &terminal, 120u, 40u);
  if (result != GHOSTTY_SUCCESS) {
    return ghostty_result_code(result);
  }

  result = ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES, NULL);
  if (result != GHOSTTY_SUCCESS) {
    ghostty_terminal_free(terminal);
    return ghostty_result_code(result);
  }
  result = ghostty_terminal_set(
      terminal,
      GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
      &scrollback_max_lines);
  if (result != GHOSTTY_SUCCESS) {
    ghostty_terminal_free(terminal);
    return ghostty_result_code(result);
  }

  *out_terminal = terminal;
  return TMON_GHOSTTY_SHIM_OK;
}

void tmon_ghostty_terminal_free(void* terminal) {
  ghostty_terminal_free((GhosttyTerminal)terminal);
}

int tmon_ghostty_terminal_feed(void* terminal, const uint8_t* data, size_t len) {
  if (terminal == NULL || (data == NULL && len != 0u)) {
    return TMON_GHOSTTY_SHIM_INVALID_ARGUMENT;
  }
  ghostty_terminal_vt_write((GhosttyTerminal)terminal, data, len);
  return TMON_GHOSTTY_SHIM_OK;
}

int tmon_ghostty_terminal_query_state(void* terminal, TmonGhosttyTerminalState* out_state) {
  if (terminal == NULL || out_state == NULL) {
    return TMON_GHOSTTY_SHIM_INVALID_ARGUMENT;
  }

  GhosttyTerminalScrollbar scrollbar = {0};
  bool cursor_visible = false;
  GhosttyResult result = ghostty_terminal_get(
      (GhosttyTerminal)terminal,
      GHOSTTY_TERMINAL_DATA_SCROLLBAR,
      &scrollbar);
  if (result != GHOSTTY_SUCCESS) {
    return ghostty_result_code(result);
  }
  result = ghostty_terminal_get(
      (GhosttyTerminal)terminal,
      GHOSTTY_TERMINAL_DATA_CURSOR_X,
      &out_state->cursor_x);
  if (result != GHOSTTY_SUCCESS) {
    return ghostty_result_code(result);
  }
  result = ghostty_terminal_get(
      (GhosttyTerminal)terminal,
      GHOSTTY_TERMINAL_DATA_CURSOR_Y,
      &out_state->cursor_y);
  if (result != GHOSTTY_SUCCESS) {
    return ghostty_result_code(result);
  }
  result = ghostty_terminal_get(
      (GhosttyTerminal)terminal,
      GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE,
      &cursor_visible);
  if (result != GHOSTTY_SUCCESS) {
    return ghostty_result_code(result);
  }

  out_state->total = scrollbar.total;
  out_state->offset = scrollbar.offset;
  out_state->len = scrollbar.len;
  out_state->cursor_visible = cursor_visible ? 1u : 0u;
  memset(out_state->_padding, 0, sizeof(out_state->_padding));
  return TMON_GHOSTTY_SHIM_OK;
}

int tmon_ghostty_terminal_compress_full(void* terminal, int* out_status) {
  if (terminal == NULL || out_status == NULL) {
    return TMON_GHOSTTY_SHIM_INVALID_ARGUMENT;
  }

  GhosttyTerminalCompressionResult result_value = GHOSTTY_TERMINAL_COMPRESSION_RESULT_COMPLETE;
  GhosttyResult result = ghostty_terminal_compress(
      (GhosttyTerminal)terminal,
      GHOSTTY_TERMINAL_COMPRESSION_MODE_FULL,
      &result_value);
  if (result != GHOSTTY_SUCCESS) {
    *out_status = TMON_GHOSTTY_COMPRESSION_ERROR;
    return TMON_GHOSTTY_SHIM_OK;
  }

  switch (result_value) {
    case GHOSTTY_TERMINAL_COMPRESSION_RESULT_COMPLETE:
      *out_status = TMON_GHOSTTY_COMPRESSION_COMPLETE;
      return TMON_GHOSTTY_SHIM_OK;
    case GHOSTTY_TERMINAL_COMPRESSION_RESULT_UNSUPPORTED:
      *out_status = TMON_GHOSTTY_COMPRESSION_UNSUPPORTED;
      return TMON_GHOSTTY_SHIM_OK;
    default:
      *out_status = TMON_GHOSTTY_COMPRESSION_ERROR;
      return TMON_GHOSTTY_SHIM_OK;
  }
}

int tmon_ghostty_terminal_semantic_digest(void* terminal, uint64_t* out_digest) {
  if (terminal == NULL || out_digest == NULL) {
    return TMON_GHOSTTY_SHIM_INVALID_ARGUMENT;
  }

  size_t total_rows = 0u;
  uint16_t cols = 0u;
  GhosttyResult result = ghostty_terminal_get(
      (GhosttyTerminal)terminal,
      GHOSTTY_TERMINAL_DATA_TOTAL_ROWS,
      &total_rows);
  if (result != GHOSTTY_SUCCESS) {
    return ghostty_result_code(result);
  }
  result = ghostty_terminal_get((GhosttyTerminal)terminal, GHOSTTY_TERMINAL_DATA_COLS, &cols);
  if (result != GHOSTTY_SUCCESS) {
    return ghostty_result_code(result);
  }
  if (cols != 120u) {
    return TMON_GHOSTTY_SHIM_GHOSTTY_ERROR;
  }

  size_t sampled_rows[64];
  const size_t sampled_len = select_sample_rows(total_rows, sampled_rows);
  Fnv1a64 hash;
  fnv1a64_init(&hash);
  fnv1a64_write_u64(&hash, total_rows);
  fnv1a64_write_u16(&hash, cols);
  fnv1a64_write_u64(&hash, sampled_len);

  for (size_t i = 0; i < sampled_len; i++) {
    const size_t row = sampled_rows[i];
    fnv1a64_write_u64(&hash, row);
    for (uint16_t col = 0u; col < cols; col++) {
      GhosttyPoint point = {
          .tag = GHOSTTY_POINT_TAG_SCREEN,
          .value.coordinate = {
              .x = col,
              .y = (uint32_t)row,
          },
      };
      GhosttyGridRef ref = GHOSTTY_INIT_SIZED(GhosttyGridRef);
      result = ghostty_terminal_grid_ref((GhosttyTerminal)terminal, point, &ref);
      if (result != GHOSTTY_SUCCESS) {
        return ghostty_result_code(result);
      }
      int cell_result = hash_cell(&hash, &ref);
      if (cell_result != TMON_GHOSTTY_SHIM_OK) {
        return cell_result;
      }
    }
  }

  *out_digest = hash.state;
  return TMON_GHOSTTY_SHIM_OK;
}

int tmon_ghostty_current_ri_phys_footprint(uint64_t* out_bytes) {
  if (out_bytes == NULL) {
    return TMON_GHOSTTY_SHIM_INVALID_ARGUMENT;
  }

  struct rusage_info_v4 usage;
  memset(&usage, 0, sizeof(usage));
  const int rc = proc_pid_rusage(getpid(), RUSAGE_INFO_V4, (rusage_info_t*)&usage);
  if (rc != 0) {
    return TMON_GHOSTTY_SHIM_PROC_ERROR;
  }

  *out_bytes = usage.ri_phys_footprint;
  return TMON_GHOSTTY_SHIM_OK;
}
