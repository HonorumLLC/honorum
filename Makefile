.DEFAULT_GOAL := help
.PHONY: help setup hooks build build-web build-api check lint test-server postgres postgres-stop perf test-ios verify-ios-release archive-ios upload-ios demo dev dev-api

CARGO ?= cargo

help:
	@echo "make setup        Install dependencies and the pre-push checks"
	@echo "make hooks        Point git at the repository's hooks"
	@echo "make build        Build the web client and the standalone server (bin/honorum)"
	@echo "make build-api    Build the API-only server without web assets (bin/honorum-api)"
	@echo "make check        Run backend, web, edge, and browser checks"
	@echo "make lint         Check Rust formatting and Clippy warnings"
	@echo "make test-server  Run the server and adapter tests against local PostgreSQL"
	@echo "make postgres     Start local PostgreSQL with Docker or Podman"
	@echo "make postgres-stop  Stop local PostgreSQL; keep its data"
	@echo "make dev-api      Run the fixture backend for Vite on port 8080"
	@echo "make perf         Measure load, sync and Canvas times on a production build"
	@echo "make test-ios     Run the iOS test suite on a simulator (macOS)"
	@echo "make verify-ios-release  Build and verify an unsigned device archive (macOS)"
	@echo "make archive-ios  Build a signed archive (needs an Apple team)"
	@echo "make upload-ios   Build and upload to TestFlight (needs an Apple team)"
	@echo "make demo         Build and run the demo at http://localhost:18080"
	@echo "make dev          Start Vite; run the API on port 8080 separately"

# Hooks first: they need no toolchain, so they install even where Node does not.
setup: hooks
	npm --prefix apps/web ci
	npm --prefix deploy/cloudflare ci

hooks:
	git config core.hooksPath scripts/githooks

build-web:
	npm --prefix apps/web run build

# The standalone server embeds apps/web/dist, so the web build comes first.
build: build-web
	$(CARGO) build --release --locked -p honorum
	mkdir -p bin
	cp target/release/honorum bin/honorum

build-api:
	$(CARGO) build --release --locked -p honorum --no-default-features
	mkdir -p bin
	cp target/release/honorum bin/honorum-api

lint:
	$(CARGO) fmt --all --check
	$(CARGO) clippy --workspace --all-targets --all-features --locked -- -D warnings
	$(CARGO) clippy -p honorum --no-default-features --locked -- -D warnings

check: build-web lint
	npm --prefix apps/web test
	npm --prefix apps/web run check
	npm --prefix apps/web run check:size
	npm --prefix deploy/cloudflare test
	npm --prefix deploy/cloudflare run check
	./scripts/postgres.sh test $(CARGO) test --workspace --all-features --locked
	$(MAKE) build-api
	cd apps/web && npx playwright test

# Each server test creates and drops an isolated database.
test-server:
	./scripts/postgres.sh test $(CARGO) test --workspace --all-features --locked

postgres:
	./scripts/postgres.sh start

postgres-stop:
	./scripts/postgres.sh stop

perf: build-web
	./scripts/postgres.sh test npm --prefix apps/web run perf

# Native builds need macOS and Xcode.
test-ios:
	xcodebuild test -project apps/ios/Honorum.xcodeproj -scheme Honorum \
	  -destination 'platform=iOS Simulator,name=iPhone 17 Pro'

verify-ios-release:
	./apps/ios/scripts/release.sh verify

archive-ios:
	./apps/ios/scripts/release.sh archive

upload-ios:
	./apps/ios/scripts/release.sh upload

demo: build
	./scripts/postgres.sh demo ./bin/honorum -config apps/web/e2e/config.json -data data/demo -addr :18080

dev:
	npm --prefix apps/web run dev

# Fixture backend on the port Vite proxies to.
dev-api: build
	./scripts/postgres.sh demo ./bin/honorum -config apps/web/e2e/config.json -data data/demo -addr :8080
