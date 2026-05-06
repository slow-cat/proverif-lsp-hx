SHELL := /bin/bash
CC ?= cc

ROOT_DIR := $(CURDIR)
TREE_SITTER_DIR := $(CURDIR)/tree-sitter-proverif
TREE_SITTER_BUILD_DIR := $(TREE_SITTER_DIR)/build
RUNTIME_DIR := $(ROOT_DIR)/runtime
RUNTIME_GRAMMARS_DIR := $(RUNTIME_DIR)/grammars
RUNTIME_QUERIES_DIR := $(RUNTIME_DIR)/queries/proverif
RUNTIME_SO := $(RUNTIME_GRAMMARS_DIR)/proverif.so
RUNTIME_HIGHLIGHTS := $(RUNTIME_QUERIES_DIR)/highlights.scm

.PHONY: help build rust-build check ts-generate ts-build runtime clean

help:
	@echo "Targets:"
	@echo "  make build        - cargo build + runtime artifacts"
	@echo "  make rust-build   - cargo build"
	@echo "  make check        - cargo check"
	@echo "  make ts-generate  - regenerate parser.c/scanner.c from grammar.js"
	@echo "  make ts-build     - build tree-sitter shared library into ./runtime/grammars with $(CC)"
	@echo "  make runtime      - build proverif.so and install highlights.scm into ./runtime"
	@echo "  make clean        - remove generated objects and runtime artifacts"

build: rust-build runtime

rust-build:
	cargo build --release

check:
	cargo check

ts-generate:
	cd "$(TREE_SITTER_DIR)" && ./node_modules/.bin/tree-sitter generate

ts-build:
	mkdir -p "$(RUNTIME_GRAMMARS_DIR)" "$(TREE_SITTER_BUILD_DIR)"
	$(CC) -fPIC -I"$(TREE_SITTER_DIR)/src" -I"$(TREE_SITTER_DIR)/src/tree_sitter" -c "$(TREE_SITTER_DIR)/src/parser.c" -o "$(TREE_SITTER_BUILD_DIR)/parser.o"
	$(CC) -fPIC -I"$(TREE_SITTER_DIR)/src" -I"$(TREE_SITTER_DIR)/src/tree_sitter" -c "$(TREE_SITTER_DIR)/src/scanner.c" -o "$(TREE_SITTER_BUILD_DIR)/scanner.o"
	$(CC) -shared "$(TREE_SITTER_BUILD_DIR)/parser.o" "$(TREE_SITTER_BUILD_DIR)/scanner.o" -o "$(RUNTIME_SO)"

runtime: ts-build
	mkdir -p "$(RUNTIME_QUERIES_DIR)"
	cp --remove-destination "$(TREE_SITTER_DIR)/queries/proverif/highlights.scm" "$(RUNTIME_HIGHLIGHTS)"

clean:
	rm -f "$(RUNTIME_SO)" "$(RUNTIME_HIGHLIGHTS)"
	rm -rf "$(TREE_SITTER_BUILD_DIR)"
	
