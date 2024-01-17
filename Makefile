.PHONY: all
all: check test build-api-docs rpm docker-build build-functional-test

.PHONY: check
check:
	cargo check --all-features --tests
	cargo clippy --all-features --tests -- -D warnings
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
	grcov . --binary-path ./target/coverage/debug/deps/ -s . -t html,lcov,covdir,cobertura --branch --ignore-not-existing --ignore '../*' --ignore "/*" --ignore "docbuilder/*" --ignore "target/*" -o target/coverage
	jq .coveragePercent target/coverage/covdir

.PHONY: coverage
coverage: ut-coverage coverage-report

.PHONY: download-osmodifier
EMU_PACKAGE_NAME ?= osmodifier_preview
EMU_PACKAGE_VERSION ?= 0.1.0-preview.473295

download-osmodifier:
	@az artifacts universal download \
	    --organization "https://dev.azure.com/mariner-org/" \
	    --project "36d030d6-1d99-4ebd-878b-09af1f4f722f" \
	    --scope project \
	    --feed "MarinerCoreArtifacts" \
	    --name '$(EMU_PACKAGE_NAME)' \
	    --version '$(EMU_PACKAGE_VERSION)' \
	    --path artifacts/
	@chmod +x artifacts/osmodifier
	@touch artifacts/osmodifier

.PHONY: rpm download-osmodifier
rpm: download-osmodifier
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

.PHONY: docker-build download-osmodifier
docker-build: download-osmodifier
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
validate-api-schema: build-api-schema docbuilder
	@echo ""
	@echo "Validating Trident API schema..."
	@diff $(TRIDENT_API_HC_SCHEMA_CHECKED_IN) $(TRIDENT_API_HC_SCHEMA_GENERATED) || { \
		echo "ERROR: Trident API schema is not up to date. Please run 'make build-api-docs' and commit the changes."; \
		exit 1; \
	}
	@echo "Trident API Schema is OK!"


.PHONY: build-functional-tests
build-functional-test:
	cargo build --tests --features functional-tests --all

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
		cargo build --target-dir $(TRIDENT_COVERAGE_TARGET) --lib --tests --features functional-tests --all --message-format=json > $(BUILD_OUTPUT)

	# Output this in case there were build failures
	@OPENSSL_STATIC=1 OPENSSL_LIB_DIR=$(shell dirname `whereis libssl.a | cut -d" " -f2`) \
		OPENSSL_INCLUDE_DIR=/usr/include/openssl \
		CARGO_INCREMENTAL=0 RUSTFLAGS='-Cinstrument-coverage' \
		LLVM_PROFILE_FILE='target/coverage/profraw/cargo-test-%p-%m.profraw' \
		cargo build --target-dir $(TRIDENT_COVERAGE_TARGET) --lib --tests --features functional-tests --all

.PHONY: functional-test
functional-test: build-functional-test-cc generate-pytest-wrappers
	cp ../k8s-tests/tools/marinerhci_test_tools/node_interface.py functional_tests/
	cp ../k8s-tests/tools/marinerhci_test_tools/ssh_node.py functional_tests/
	python3 -u -m pytest functional_tests/ --setup-show -vv -o junit_logging=all --junitxml $(FUNCTIONAL_TEST_JUNIT_XML) ${EXTRA_PARAMS} --keep-environment --test-dir $(FUNCTIONAL_TEST_DIR) --build-output $(BUILD_OUTPUT) --force-upload # -k test_osutils -s

.PHONY: patch-functional-test
patch-functional-test: build-functional-test-cc generate-pytest-wrappers
	python3 -u -m pytest functional_tests/ --setup-show -vv -o junit_logging=all --junitxml $(FUNCTIONAL_TEST_JUNIT_XML) ${EXTRA_PARAMS} --keep-environment --test-dir $(FUNCTIONAL_TEST_DIR) --build-output $(BUILD_OUTPUT) --reuse-environment -s # -k test_osutils -s

.PHONY: generate-pytest-wrappers
generate-pytest-wrappers:
	rm -rf functional_tests/generated/*
	cargo build --features=pytest-generator,functional-tests
	target/debug/trident pytest