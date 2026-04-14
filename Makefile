.DEFAULT_GOAL := help
SHELL := /bin/sh

.PHONY: help init build release fmt test install install-global update sync-code uninstall purge sync-config unsync-config paths

help:
	@printf '%s\n' \
		'make init            Install Rust if needed and build a local release binary' \
		'make build           Build the project in debug mode' \
		'make release         Build the project in release mode' \
		'make fmt             Format the Rust code' \
		'make test            Run the Rust test suite' \
		'make install         Install acs globally from committed local HEAD or a specified ref' \
		'make update          Update the global acs binary from latest tag or a specified ref' \
		'make sync-code       Reinstall the global acs binary from the current checkout' \
		'make sync-config     Sync local config/ or acs.config.json into the global config overlay' \
		'make unsync-config   Remove the synced local config overlay from the global config dir' \
		'make uninstall       Uninstall the global acs binary and keep user config/logs' \
		'make purge           Uninstall the binary and remove standard config/logs' \
		'make paths           Print the standard config/log/bin directories' \
		'ref vars             Use TAG=..., BRANCH=..., or COMMIT=... with make install/update'

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
	@./scripts/install.sh $(if $(TAG),--tag $(TAG),) $(if $(BRANCH),--branch $(BRANCH),) $(if $(COMMIT),--commit $(COMMIT),)

update:
	@./scripts/update.sh $(if $(TAG),--tag $(TAG),) $(if $(BRANCH),--branch $(BRANCH),) $(if $(COMMIT),--commit $(COMMIT),)

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

paths:
	@./scripts/paths.sh
