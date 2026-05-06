# proverif-lsp-hx

Minimal ProVerif language server in Rust for helix editor.

Current features:

- full document sync
- syntax diagnostics from tree-sitter error nodes
- keyword completion
- basic hover on declaration identifiers

The parser sources are vendored in `tree-sitter-proverif/`.

Useful commands:

- `make build` for Rust build + runtime artifacts
- `make check` for Rust build checks
- `make ts-generate` to regenerate parser sources from `grammar.js`
- `make runtime` to build `proverif.so` and copy it (plus `highlights.scm`) into `./runtime/`
