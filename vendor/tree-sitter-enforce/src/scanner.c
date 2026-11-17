#include "tree_sitter/parser.h"

#include <wctype.h>

enum TokenType {
    AUTOMATIC_SEMICOLON,
    SIZE_BRACKET,
};

void *tree_sitter_enforce_external_scanner_create() { return NULL; }

void tree_sitter_enforce_external_scanner_destroy(void *payload) { (void)payload; }

unsigned tree_sitter_enforce_external_scanner_serialize(void *payload, char *buffer) {
    (void)payload;
    (void)buffer;
    return 0;
}

void tree_sitter_enforce_external_scanner_deserialize(void *payload, const char *buffer,
                                                      unsigned length) {
    (void)payload;
    (void)buffer;
    (void)length;
}

static inline void skip(TSLexer *lexer) { lexer->advance(lexer, true); }

typedef enum {
    REJECT,
    NO_NEWLINE,
    ACCEPT,
} WhitespaceResult;

/**
 * @param consume If false, only consume enough to check whether a block comment
 * spans a newline.
 */
static WhitespaceResult scan_whitespace_and_comments(TSLexer *lexer, bool consume) {
    bool saw_block_newline = false;

    for (;;) {
        while (iswspace(lexer->lookahead)) {
            skip(lexer);
        }

        if (lexer->lookahead != '/') {
            return ACCEPT;
        }
        skip(lexer);

        if (lexer->lookahead == '/') {
            skip(lexer);
            while (lexer->lookahead != 0 && lexer->lookahead != '\n') {
                skip(lexer);
            }
        } else if (lexer->lookahead == '*') {
            skip(lexer);
            while (lexer->lookahead != 0) {
                if (lexer->lookahead == '*') {
                    skip(lexer);
                    if (lexer->lookahead == '/') {
                        skip(lexer);
                        if (lexer->lookahead != '/' && !consume) {
                            return saw_block_newline ? ACCEPT : NO_NEWLINE;
                        }
                        break;
                    }
                } else if (lexer->lookahead == '\n') {
                    saw_block_newline = true;
                    skip(lexer);
                } else {
                    skip(lexer);
                }
            }
        } else {
            return REJECT;
        }
    }
}

static bool scan_automatic_semicolon(TSLexer *lexer) {
    lexer->result_symbol = AUTOMATIC_SEMICOLON;

    for (;;) {
        if (lexer->lookahead == 0) {
            return true;
        }

        if (lexer->lookahead == '}') {
            return true;
        }

        if (lexer->is_at_included_range_start(lexer)) {
            return true;
        }

        if (lexer->lookahead == '/') {
            WhitespaceResult result = scan_whitespace_and_comments(lexer, false);
            if (result == REJECT) {
                return false;
            }
            if (result == ACCEPT) {
                break;
            }
            continue;
        }

        if (lexer->lookahead == '\n') {
            skip(lexer);
            break;
        }

        if (!iswspace(lexer->lookahead)) {
            return false;
        }

        skip(lexer);
    }

    if (scan_whitespace_and_comments(lexer, true) == REJECT) {
        return false;
    }

    switch (lexer->lookahead) {
        // A `{` on a new line opens a block or continues a literal; it never ends a statement.
        case '{':
        case ',':
        case ':':
        case ';':
        case '*':
        case '%':
        case '>':
        case '<':
        case '=':
        case '[':
        case '(':
        case '?':
        case '^':
        case '|':
        case '&':
        case '/':
            return false;

        // A `.` continues an expression unless it starts a decimal literal.
        case '.':
            skip(lexer);
            return iswdigit(lexer->lookahead);

        // Insert a semicolon before `--` and `++`, but not before a binary `+` or `-`.
        case '+':
            skip(lexer);
            return lexer->lookahead == '+';
        case '-':
            skip(lexer);
            return lexer->lookahead == '-';

        // Insert one before a unary `!` but not before `!=`.
        case '!':
            skip(lexer);
            return lexer->lookahead != '=';

        default:
            return true;
    }
}

static bool scan_size_bracket(TSLexer *lexer) {
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t' || lexer->lookahead == '\r') {
        skip(lexer);
    }
    if (lexer->lookahead != '[') {
        return false;
    }
    lexer->advance(lexer, false);
    lexer->result_symbol = SIZE_BRACKET;
    lexer->mark_end(lexer);
    return true;
}

bool tree_sitter_enforce_external_scanner_scan(void *payload, TSLexer *lexer,
                                               const bool *valid_symbols) {
    (void)payload;
    // Pinned before any whitespace is skipped, so the zero-width `;` ends where the
    // statement does, not after the spaces or `\r` a size bracket looked past.
    lexer->mark_end(lexer);
    if (valid_symbols[SIZE_BRACKET] && scan_size_bracket(lexer)) {
        return true;
    }
    if (!valid_symbols[AUTOMATIC_SEMICOLON]) {
        return false;
    }
    return scan_automatic_semicolon(lexer);
}
