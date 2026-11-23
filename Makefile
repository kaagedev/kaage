.DEFAULT_GOAL := help

# Automatically enter `nix develop` when running on NixOS outside a dev shell.
HAS_CC := $(shell command -v cc >/dev/null 2>&1 && echo yes || echo no)
HAS_NIX := $(shell command -v nix >/dev/null 2>&1 && echo yes || echo no)
ifeq ($(HAS_CC),no)
  ifeq ($(HAS_NIX),yes)
    RUN := nix develop --command
  endif
endif

KAAGE_BIND ?= 127.0.0.1:9090
KAAGE_DIST_DIR ?= dist

.PHONY: help build build-nix test run run-ui run-native clean

help: ## Show available Makefile targets
	@echo "kaage (ಕಾಗೆ / 影) — Build & Development Targets"
	@echo ""
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z0-9_-]+:.*## / {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)
	@echo ""
	@echo "Environment overrides:"
	@echo "  KAAGE_BIND=$(KAAGE_BIND)"
	@echo "  KAAGE_DIST_DIR=$(KAAGE_DIST_DIR)"

build: ## Build kaaged daemon and WASM webtop bundle (dist/)
	$(RUN) cargo build -p kaaged
	$(RUN) trunk build --release

build-nix: ## Build the reproducible Nix flake package (.#kaage)
	nix build .#kaage

test: ## Run workspace unit tests and verify WASM target compiles
	$(RUN) cargo test --workspace
	$(RUN) cargo check --target wasm32-unknown-unknown -p kaage-ui

run: ## Build WASM UI and run kaaged locally (http://127.0.0.1:9090)
	$(RUN) trunk build
	KAAGE_BIND=$(KAAGE_BIND) KAAGE_DIST_DIR=$(KAAGE_DIST_DIR) $(RUN) cargo run -p kaaged

run-ui: ## Serve live-reloading WASM UI via trunk (http://127.0.0.1:8080)
	$(RUN) trunk serve

run-native: ## Launch the native desktop kaage-ui client
	$(RUN) cargo run -p kaage-ui

clean: ## Remove build artifacts (target/, dist/, result)
	rm -rf target dist result
