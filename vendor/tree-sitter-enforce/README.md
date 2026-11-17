# tree-sitter-enforce (vendored)

Copied from https://github.com/simonvic/tree-sitter-enforce at commit
`f6daeccf744141c13b4a94c39d7de22307173735` (2026-09-16), MIT licence in `LICENSE`. Only the Rust binding, the
grammar, its queries, generated sources and tests are kept. ModFoundry changes `grammar.js` here and regenerates
`src/` with the tree-sitter CLI (`npx tree-sitter-cli@0.26.13 generate`); every change is listed below.

## Changes from upstream

- `ref` is a field, variable and formal-parameter modifier, so it may precede another modifier (`private ref static W s;`); `ref X` still parses as `type_ref`.
- `autoptr` is accepted before a type argument (`array<autoptr X>`), and `queries/highlights.scm` colours it as a modifier.
- `local` is accepted as a formal-parameter modifier.
- A stray `;` is accepted in a class body.
- Enum members may be separated by a line break only, and a stray `,` may precede a member's value.
- A switch body accepts statements before the first `case`, a body with no `case`, and `default` anywhere, even twice.
- A line break ends a field, a local declaration, `return`, `break`, `continue` and an expression statement: `src/scanner.c` supplies a zero-width `;` (external token), and a block consumes that token after its `}` the way tree-sitter-javascript does. `else` is reserved so the automatic `;` cannot turn it into an identifier, and a bare call statement loses to an expression statement.
- A declarator takes any number of array sizes, not one (`int g[2][3];`). A size's `[` is an external token that only matches on the declarator's own line, so a `[` on the next line still starts an attribute list.

---

# tree-sitter-enforce

[![CI][ci]](https://github.com/simonvic/tree-sitter-enforce/actions/workflows/publish.yml)

Enforce grammar for [tree-sitter](https://github.com/tree-sitter/tree-sitter).

This grammar intends to be a close approximation of the
[Enforce](https://community.bistudio.com/wiki/DayZ:Enforce_Script_Syntax)
script "specification".

> [!NOTE]
> We don't have official specification; some opinionated choices may have been
> taken (e.g. mandatory semicolon terminated statements)
> 
> Read more about it
> [here](https://github.com/simonvic/tree-sitter-enforce/issues/7)

## Getting started

### neovim

Neovim doesn't recognize the Enforce filetype, and since it uses `.c` file
extension, you need to add a custom language filetype; add the following
somewhere in your `init.lua`

```lua
vim.filetype.add({
	pattern = {
		[".*/scripts/.*/.*%.c"] = "enforce",
	},
})
```

*The pattern will match all `.c` files in a `scripts/some_module/` directory.
You may have to modify the pattern to suit your needs.*

To install the parser, you can use
[nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter) plugin
and run `:TSInstall enforce`

### VSCode

> [!NOTE]
> This has not been tested. Use at your own discretion

You can use `tree-sitter-vscode` plugin.
You'll likely need `tree-sitter-enforce.wasm`, which you can find in the
[releases](https://github.com/simonvic/tree-sitter-enforce/releases)
or you can clone this repository and build it with tree-sitter

[ci]: https://img.shields.io/github/actions/workflow/status/simonvic/tree-sitter-enforce/publish.yml?logo=github&label=CI
