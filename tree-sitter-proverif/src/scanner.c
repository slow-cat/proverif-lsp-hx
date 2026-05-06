#include <tree_sitter/parser.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

enum TokenType {
  COMMENT,
};

typedef struct {
  uint32_t depth;
} Scanner;

void *tree_sitter_proverif_external_scanner_create(void) {
  Scanner *scanner = (Scanner *)calloc(1, sizeof(Scanner));
  return scanner;
}

void tree_sitter_proverif_external_scanner_destroy(void *payload) {
  free(payload);
}

unsigned tree_sitter_proverif_external_scanner_serialize(void *payload, char *buffer) {
  Scanner *scanner = (Scanner *)payload;
  buffer[0] = (char)scanner->depth;
  return 1;
}

void tree_sitter_proverif_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
  Scanner *scanner = (Scanner *)payload;
  scanner->depth = length > 0 ? (uint8_t)buffer[0] : 0;
}

static bool scan_comment(TSLexer *lexer, Scanner *scanner) {
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t' || lexer->lookahead == '\n' || lexer->lookahead == '\r') {
    lexer->advance(lexer, true);
  }

  if (lexer->lookahead != '(') {
    return false;
  }

  lexer->advance(lexer, false);
  if (lexer->lookahead != '*') {
    return false;
  }

  lexer->advance(lexer, false);
  scanner->depth = 1;

  while (scanner->depth > 0) {
    if (lexer->eof(lexer)) {
      return false;
    }

    if (lexer->lookahead == '(') {
      lexer->advance(lexer, false);
      if (lexer->lookahead == '*') {
        lexer->advance(lexer, false);
        scanner->depth++;
        continue;
      }
      continue;
    }

    if (lexer->lookahead == '*') {
      lexer->advance(lexer, false);
      if (lexer->lookahead == ')') {
        lexer->advance(lexer, false);
        scanner->depth--;
        continue;
      }
      continue;
    }

    lexer->advance(lexer, false);
  }

  scanner->depth = 0;
  lexer->result_symbol = COMMENT;
  return true;
}

bool tree_sitter_proverif_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  Scanner *scanner = (Scanner *)payload;
  if (valid_symbols[COMMENT]) {
    return scan_comment(lexer, scanner);
  }
  return false;
}
