.DEFAULT_GOAL := help
SHELL := /bin/sh

.PHONY: help init build release fmt test install install-global sync-code uninstall purge update update-latest sync-config unsync-config paths

help:
	@printf '%s\n' \
		'make init            Install Rust if needed and build a local release binary' \
		'make build           Build the project in debug mode' \
		'make release         Build the project in release mode' \
		'make fmt             Format the Rust code' \
		'make test            Run the Rust test suite' \
		'make install         Install acs globally with standard config/log directories' \
		'make sync-code       Reinstall the global acs binary from the current checkout' \
		'make sync-config     Sync local config/ or acs.config.json into the global config overlay' \
		'make unsync-config   Remove the synced local config overlay from the global config dir' \
		'make uninstall       Uninstall the global acs binary and keep user config/logs' \
		'make purge           Uninstall the binary and remove standard config/logs' \
		'make update          Choose a GitHub tag interactively and install it globally' \
		'make update-latest   Install the latest GitHub tag globally' \
		'make paths           Print the standard config/log/bin directories'

init:
	@./scripts/init.sh

build:
	@cargo build

release:
	@cargo build --release

fmt:
	@cargo fmt

test:
	@cargo test

install: install-global

install-global:
	@./scripts/install.sh

sync-code:
	@./scripts/sync-code.sh

sync-config:
	@./scripts/sync-config.sh

unsync-config:
	@./scripts/unsync-config.sh

uninstall:
	@./scripts/uninstall.sh

purge:
	@./scripts/uninstall.sh --purge

update:
	@./scripts/update.sh $(if $(VERSION),--version $(VERSION),) $(if $(LATEST),--latest,)

update-latest:
	@./scripts/update.sh --latest

paths:
	@./scripts/paths.sh
