.DEFAULT_GOAL := help
SHELL := /bin/sh

.PHONY: help init build build-release release fmt test install install-global update sync-code uninstall purge paths

help:
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

init:
	@./scripts/init.sh

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
	@./scripts/install.sh $(if $(TAG),--tag $(TAG),) $(if $(BRANCH),--branch $(BRANCH),) $(if $(COMMIT),--commit $(COMMIT),)

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
