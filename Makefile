.PHONY: all
all: format check test build-api-docs rpm docker-build build-functional-test coverage validate-configs

.PHONY: check
check:
	cargo check --workspace --all-features --tests
	cargo clippy --version
	cargo clippy --locked --workspace -- -D warnings 2>&1
	cargo clippy --locked --workspace --all-features -- -D warnings 2>&1
	cargo clippy --locked --workspace --tests -- -D warnings 2>&1
	cargo clippy --locked --workspace --tests --all-features -- -D warnings 2>&1
	cargo fmt -- --check

.PHONY: format
format:
	cargo fmt
	python3 -m black .

.PHONY: test
test:
	cargo test --all --no-fail-fast

.PHONY: ut-coverage
ut-coverage:
	mkdir -p target/coverage/profraw
	CARGO_INCREMENTAL=0 RUSTFLAGS='-Cinstrument-coverage' LLVM_PROFILE_FILE='target/coverage/profraw/cargo-test-%p-%m.profraw' cargo test --target-dir target/coverage --all --no-fail-fast

.PHONY: coverage-report
coverage-report:
	# cargo install grcov
	grcov . --binary-path ./target/coverage/debug/deps/ -s . -t html,covdir,cobertura --branch --ignore-not-existing --ignore '../*' --ignore "/*" --ignore "docbuilder/*" --ignore "target/*" -o target/coverage
	jq .coveragePercent target/coverage/covdir

.PHONY: coverage
coverage: ut-coverage coverage-report

.PHONY: clean-coverage
clean-coverage:
	rm -rf target/coverage/profraw

EMU_PACKAGE_NAME ?= osmodifier_preview
EMU_PACKAGE_VERSION ?= 0.1.0-preview.490287
artifacts/osmodifier:
	az artifacts universal download \
		--organization "https://dev.azure.com/mariner-org/" \
		--project "36d030d6-1d99-4ebd-878b-09af1f4f722f" \
		--scope project \
		--feed "MarinerCoreArtifacts" \
		--name '$(EMU_PACKAGE_NAME)' \
		--version '$(EMU_PACKAGE_VERSION)' \
		--path artifacts/
	chmod +x artifacts/osmodifier

.PHONY: rpm
rpm: artifacts/osmodifier
	$(eval TRIDENT_CARGO_VERSION := $(shell cargo metadata --format-version 1 | jq -r '.packages[] | select(.name == "trident") | .version'))
	$(eval GIT_COMMIT := $(shell git rev-parse --short HEAD)$(shell git diff --quiet || echo '.dirty'))
	docker build --progress plain -t trident/trident-build:latest \
		--build-arg TRIDENT_VERSION="$(TRIDENT_CARGO_VERSION)-dev.$(GIT_COMMIT)" \
		--build-arg RPM_VER="$(TRIDENT_CARGO_VERSION)"\
		--build-arg RPM_REL="dev.$(GIT_COMMIT)"\
		.
	mkdir -p bin/
	id=$$(docker create trident/trident-build:latest) && \
	docker cp $$id:/work/trident.tar.gz bin/ && \
	docker rm -v $$id && \
	tar xf bin/trident.tar.gz -C bin/

SYSTEMD_RPM_TAR_URL ?= https://hermesimages.blob.core.windows.net/hermes-test/systemd-254-3.tar.gz

artifacts/systemd/systemd-254-3.cm2.x86_64.rpm:
	mkdir -p ./artifacts/systemd
	curl $(SYSTEMD_RPM_TAR_URL) | tar -xz -C ./artifacts/systemd --strip-components=1
	rm -f ./artifacts/systemd/*.src.rpm ./artifacts/systemd/systemd-debuginfo*.rpm ./artifacts/systemd/systemd-devel-*.rpm

.PHONY: docker-build
docker-build: artifacts/osmodifier artifacts/systemd/systemd-254-3.cm2.x86_64.rpm
	$(eval TRIDENT_CARGO_VERSION := $(shell cargo metadata --format-version 1 | jq -r '.packages[] | select(.name == "trident") | .version'))
	$(eval GIT_COMMIT := $(shell git rev-parse --short HEAD)$(shell git diff --quiet || echo '.dirty'))
	docker build -f Dockerfile.runtime --progress plain -t trident/trident:latest \
		--build-arg TRIDENT_VERSION="$(TRIDENT_CARGO_VERSION)-dev.$(GIT_COMMIT)" \
		--build-arg RPM_VER="$(TRIDENT_CARGO_VERSION)" \
		--build-arg RPM_REL="dev.$(GIT_COMMIT)" \
		.

.PHONY: clean
clean:
	cargo clean
	rm -rf bin/
	rm -rf artifacts/
	find . -name "*.profraw" -type f -delete

# Locally we generally want to compile in debugging mode to reuse local artifacs.
# On pipelines, though, we compile in release mode. This variable allows us to
# pass `--release` to cargo build when needed.
DOCS_RELEASE_BUILD ?= n

ifeq ($(DOCS_RELEASE_BUILD),y)
	DOCS_BIN_DIR := target/release
	DOCS_CARGO_ARGS := --release
else
	DOCS_BIN_DIR := target/debug
	DOCS_CARGO_ARGS :=
endif

.PHONY: docbuilder
docbuilder:
	cargo build --package docbuilder $(DOCS_CARGO_ARGS)
	$(eval DOCBUILDER_BIN := $(DOCS_BIN_DIR)/docbuilder)


.PHONY: setsail-docs
setsail-docs: docbuilder
	mkdir -p target/setsail-docs
	$(DOCBUILDER_BIN) setsail -o target/setsail-docs
	@echo Wrote docs to target/setsail-docs


TRIDENT_API_HC_SCHEMA_GENERATED  := target/trident-api-docs/host-config-schema.json
TRIDENT_API_HC_SCHEMA_CHECKED_IN := trident_api/schemas/host-config-schema.json

TRIDENT_API_HC_MARKDOWN_DIR := docs/Reference/Host-Configuration/API-Reference
TRIDENT_API_HC_EXAMPLE_FILE := docs/Reference/Host-Configuration/sample-host-configuration.md

target/trident-api-docs:
	mkdir -p target/trident-api-docs

.PHONY: build-api-schema
build-api-schema: target/trident-api-docs docbuilder
	$(DOCBUILDER_BIN) host-config schema -o "$(TRIDENT_API_HC_SCHEMA_GENERATED)"

.PHONY: build-api-docs
build-api-docs: build-api-schema docbuilder
	$(DOCBUILDER_BIN) host-config sample -m -o $(TRIDENT_API_HC_EXAMPLE_FILE)
	@echo Updated sample Host Configuration in $(TRIDENT_API_HC_EXAMPLE_FILE)

	cp $(TRIDENT_API_HC_SCHEMA_GENERATED) $(TRIDENT_API_HC_SCHEMA_CHECKED_IN)
	@echo Updated $(TRIDENT_API_HC_SCHEMA_CHECKED_IN)

	$(DOCBUILDER_BIN) host-config markdown $(TRIDENT_API_HC_MARKDOWN_DIR) --devops-wiki
	@echo Wrote Markdown docs to $(TRIDENT_API_HC_MARKDOWN_DIR)

.PHONY: validate-api-schema
validate-api-schema: build-api-schema docbuilder validate-hc-sample
	@echo ""
	@echo "Validating Trident API schema..."
	@diff $(TRIDENT_API_HC_SCHEMA_CHECKED_IN) $(TRIDENT_API_HC_SCHEMA_GENERATED) || { \
		echo "ERROR: Trident API schema is not up to date. Please run 'make build-api-docs' and commit the changes."; \
		exit 1; \
	}
	@echo "Trident API Schema is OK!"

.PHONY: validate-hc-sample
validate-hc-sample: build-api-docs
	$(eval TMP := $(shell mktemp -d))
	$(DOCBUILDER_BIN) host-config sample -o $(TMP)/sample-host-configuration.yaml
	cargo run validate --host-config $(TMP)/sample-host-configuration.yaml
	rm -rf $(TMP)

.PHONY: build-functional-tests
build-functional-test:
	cargo build --tests --features functional-test --all

FUNCTIONAL_TEST_DIR := /tmp/trident-test
FUNCTIONAL_TEST_JUNIT_XML := target/trident_functional_tests.xml
TRIDENT_COVERAGE_TARGET := target/coverage
BUILD_OUTPUT := $(shell mktemp)

.PHONY: build-functional-tests-cc
build-functional-test-cc:
	# Redirect output to get to the test binaries; needs to be in sync with below
	-@OPENSSL_STATIC=1 OPENSSL_LIB_DIR=$(shell dirname `whereis libssl.a | cut -d" " -f2`) \
		OPENSSL_INCLUDE_DIR=/usr/include/openssl \
		CARGO_INCREMENTAL=0 RUSTFLAGS='-Cinstrument-coverage' \
		LLVM_PROFILE_FILE='target/coverage/profraw/cargo-test-%p-%m.profraw' \
		cargo build --target-dir $(TRIDENT_COVERAGE_TARGET) --lib --tests --features functional-test --all --message-format=json > $(BUILD_OUTPUT)

	# Output this in case there were build failures
	@OPENSSL_STATIC=1 OPENSSL_LIB_DIR=$(shell dirname `whereis libssl.a | cut -d" " -f2`) \
		OPENSSL_INCLUDE_DIR=/usr/include/openssl \
		CARGO_INCREMENTAL=0 RUSTFLAGS='-Cinstrument-coverage' \
		LLVM_PROFILE_FILE='target/coverage/profraw/cargo-test-%p-%m.profraw' \
		cargo build --target-dir $(TRIDENT_COVERAGE_TARGET) --lib --tests --features functional-test --all

.PHONY: functional-test
functional-test: build-functional-test-cc generate-functional-test-manifest
	cp ../k8s-tests/tools/marinerhci_test_tools/node_interface.py functional_tests/
	cp ../k8s-tests/tools/marinerhci_test_tools/ssh_node.py functional_tests/
	python3 -u -m pytest functional_tests/$(FILTER) -v -o junit_logging=all --junitxml $(FUNCTIONAL_TEST_JUNIT_XML) ${FUNCTIONAL_TEST_EXTRA_PARAMS} --keep-environment --test-dir $(FUNCTIONAL_TEST_DIR) --build-output $(BUILD_OUTPUT) --force-upload

.PHONY: patch-functional-test
patch-functional-test: build-functional-test-cc generate-functional-test-manifest
	python3 -u -m pytest functional_tests/$(FILTER) -v -o junit_logging=all --junitxml $(FUNCTIONAL_TEST_JUNIT_XML) ${FUNCTIONAL_TEST_EXTRA_PARAMS} --keep-environment --test-dir $(FUNCTIONAL_TEST_DIR) --build-output $(BUILD_OUTPUT) --reuse-environment

.PHONY: generate-functional-test-manifest
generate-functional-test-manifest:
	rm -rf functional_tests/generated/*
	cargo build --features=pytest-generator,functional-test
	target/debug/trident pytest

.PHONY: validate-configs
validate-configs:
	@cargo build
	$(eval DETECTED_HC_FILES := $(shell grep -R 'hostConfiguration:' . --include '*.yaml' --exclude-dir=target --exclude-dir=dev -l))
	@for file in $(DETECTED_HC_FILES); do \
		echo "Validating $$file"; \
		./target/debug/trident validate -c $$file; \
	done
