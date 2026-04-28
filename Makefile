.DEFAULT_GOAL := help

ifeq ($(OS),Windows_NT)
SHELL := cmd
.SHELLFLAGS := /C
INIT_SCRIPT := powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/init.ps1
INSTALL_SCRIPT := powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install.ps1
INSTALL_ARGS := $(if $(TAG),--tag $(TAG),) $(if $(BRANCH),--branch $(BRANCH),) $(if $(COMMIT),--commit $(COMMIT),)
else
SHELL := /bin/sh
INIT_SCRIPT := ./scripts/init.sh
INSTALL_SCRIPT := ./scripts/install.sh
INSTALL_ARGS := $(if $(TAG),--tag $(TAG),) $(if $(BRANCH),--branch $(BRANCH),) $(if $(COMMIT),--commit $(COMMIT),)
endif

.PHONY: help init build build-release release fmt test install install-global update sync-code uninstall purge paths

help:
ifeq ($(OS),Windows_NT)
	@echo make init            必要なら Rust を導入し、ローカルの release バイナリをビルド
	@echo make build           デバッグモードでプロジェクトをビルド
	@echo make build-release   release モードでプロジェクトをビルド
	@echo make release         バージョン更新、テスト、ビルド、コミット、タグ付けを実行
	@echo make fmt             Rust コードを整形
	@echo make test            Rust のテスト一式を実行
	@echo make install         コミット済みのローカル HEAD または指定 ref から acs をグローバル導入
	@echo make update          最新タグまたは指定 ref からグローバル acs を更新
	@echo make sync-code       現在のチェックアウトからグローバル acs を再インストール
	@echo make uninstall       グローバル acs をアンインストールし、ユーザーログは残す
	@echo make purge           バイナリを削除し、標準ログも削除
	@echo make paths           標準のログ/バイナリ配置先を表示
	@echo ref vars             make install/update では TAG=..., BRANCH=..., COMMIT=... を使用可能
else
	@printf '%s\n' \
		'make init            必要なら Rust を導入し、ローカルの release バイナリをビルド' \
		'make build           デバッグモードでプロジェクトをビルド' \
		'make build-release   release モードでプロジェクトをビルド' \
		'make release         バージョン更新、テスト、ビルド、コミット、タグ付けを実行' \
		'make fmt             Rust コードを整形' \
		'make test            Rust のテスト一式を実行' \
		'make install         コミット済みのローカル HEAD または指定 ref から acs をグローバル導入' \
		'make update          最新タグまたは指定 ref からグローバル acs を更新' \
		'make sync-code       現在のチェックアウトからグローバル acs を再インストール' \
		'make uninstall       グローバル acs をアンインストールし、ユーザーログは残す' \
		'make purge           バイナリを削除し、標準ログも削除' \
		'make paths           標準のログ/バイナリ配置先を表示' \
		'ref vars             make install/update では TAG=..., BRANCH=..., COMMIT=... を使用可能'
endif

init:
	@$(INIT_SCRIPT)

build:
	@cargo build

build-release:
	@cargo build --release

release:
	@./scripts/release.sh --version $(VERSION)

fmt:
	@cargo fmt

test:
	@cargo test

install: install-global

install-global:
	@$(INSTALL_SCRIPT) $(INSTALL_ARGS)

update:
	@./scripts/update.sh $(if $(TAG),--tag $(TAG),) $(if $(BRANCH),--branch $(BRANCH),) $(if $(COMMIT),--commit $(COMMIT),)

sync-code:
	@./scripts/sync-code.sh

uninstall:
	@./scripts/uninstall.sh

purge:
	@./scripts/uninstall.sh --purge

paths:
	@./scripts/paths.sh
