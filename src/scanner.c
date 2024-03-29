#include "tree_sitter/alloc.h"
#include "tree_sitter/array.h"
#include "tree_sitter/parser.h"

enum TokenType {
  NEWLINE,
  INDENT,
  DEDENT,
  BLOCK_COMMENT_CONTENT,
  LINE_COMMENT,
  ERROR_SENTINEL
};

typedef struct {
  Array(uint16_t) indents;
} Scanner;

static inline void advance(TSLexer *lexer) { lexer->advance(lexer, false); }

static inline void skip(TSLexer *lexer) { lexer->advance(lexer, true); }

static inline bool scan_block_comment(TSLexer *lexer) {
  lexer->mark_end(lexer);
  if (lexer->lookahead != '(')
    return false;

  advance(lexer);
  if (lexer->lookahead != '*')
    return false;

  advance(lexer);

  while (true) {
    switch (lexer->lookahead) {
    case '(':
      scan_block_comment(lexer);
      break;
    case '*':
      advance(lexer);
      if (lexer->lookahead == ')') {
        advance(lexer);
        return true;
      }
      break;
    case '\0':
      return true;
    default:
      advance(lexer);
    }
  }
}

static inline bool is_infix_op_start(TSLexer *lexer) {
  switch (lexer->lookahead) {
  case '+':
  case '-':
  case '%':
  case '&':
  case '|':
  case '=':
  case ':':
  case '$':
  case '?':
  case '@':
  case '.':
  case '!':
  case '/':
  case '<':
  case '>':
  case '^':
  case '~':
    return true;
  case 'o':
    skip(lexer);
    return lexer->lookahead == 'r';
  default:
    return false;
  }
}

bool tree_sitter_fsharp_external_scanner_scan(void *payload, TSLexer *lexer,
                                              const bool *valid_symbols) {
  Scanner *scanner = (Scanner *)payload;

  bool error_recovery_mode = valid_symbols[ERROR_SENTINEL];

  lexer->mark_end(lexer);

  bool found_end_of_line = false;
  bool found_start_of_infix_op = false;
  uint32_t indent_length = lexer->get_column(lexer);

  for (;;) {
    if (lexer->lookahead == '\n') {
      found_end_of_line = true;
      indent_length = 0;
      skip(lexer);
    } else if (lexer->lookahead == ' ') {
      indent_length++;
      skip(lexer);
    } else if (lexer->lookahead == '\r' || lexer->lookahead == '\f') {
      indent_length = 0;
      skip(lexer);
    } else if (lexer->lookahead == '\t') {
      indent_length += 8;
      skip(lexer);
    } else if (lexer->eof(lexer)) {
      indent_length = 0;
      found_end_of_line = true;
      break;
    } else if (is_infix_op_start(lexer)) {
      found_start_of_infix_op = true;
      break;
    } else {
      break;
    }
  }

  if (scanner->indents.size > 0) {
    uint16_t current_indent_length = *array_back(&scanner->indents);

    if (valid_symbols[INDENT] && indent_length > current_indent_length &&
        !found_start_of_infix_op) {
      array_push(&scanner->indents, indent_length);
      lexer->result_symbol = INDENT;
      return true;
    }

    if (found_end_of_line) {
      if ((valid_symbols[DEDENT] || (!valid_symbols[NEWLINE])) &&
          indent_length < current_indent_length) {
        array_pop(&scanner->indents);
        lexer->result_symbol = DEDENT;
        return true;
      }
    }
  }

  if (found_end_of_line) {
    if (valid_symbols[NEWLINE] && !error_recovery_mode) {
      lexer->result_symbol = NEWLINE;
      return true;
    }
  }

  return false;
}

unsigned tree_sitter_fsharp_external_scanner_serialize(void *payload,
                                                       char *buffer) {
  Scanner *scanner = (Scanner *)payload;
  size_t size = 0;

  uint32_t iter = 1;
  for (; iter < scanner->indents.size &&
         size < TREE_SITTER_SERIALIZATION_BUFFER_SIZE;
       ++iter) {
    buffer[size++] = (char)*array_get(&scanner->indents, iter);
  }

  return size;
}

void tree_sitter_fsharp_external_scanner_deserialize(void *payload,
                                                     const char *buffer,
                                                     unsigned length) {
  Scanner *scanner = (Scanner *)payload;

  array_delete(&scanner->indents);
  array_push(&scanner->indents, 0);

  if (length > 0) {
    size_t size = 0;

    for (; size < length; size++) {
      array_push(&scanner->indents, (unsigned char)buffer[size]);
    }
  }
}

void *tree_sitter_fsharp_external_scanner_create() {
  Scanner *scanner = ts_calloc(1, sizeof(Scanner));
  array_init(&scanner->indents);
  tree_sitter_fsharp_external_scanner_deserialize(scanner, NULL, 0);
  return scanner;
}

void tree_sitter_fsharp_external_scanner_destroy(void *payload) {
  Scanner *scanner = (Scanner *)payload;
  array_delete(&scanner->indents);
  ts_free(scanner);
}
