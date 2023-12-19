.PHONY: all
all: check build test rpm build-api-docs docker-build

.PHONY: check
check:
	cargo check
	cargo clippy -- -D warnings
	cargo fmt -- --check

.PHONY: build
build:
	cargo build

.PHONY: build-release
build-release:
	cargo build --release

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
	grcov . --binary-path ./target/coverage/debug/deps/ -s . -t html,lcov,covdir,cobertura --branch --ignore-not-existing --ignore '../*' --ignore "/*" -o target/coverage
	jq .coveragePercent target/coverage/covdir

.PHONY: coverage
coverage: ut-coverage coverage-report

.PHONY: rpm
rpm:
	$(eval TRIDENT_CARGO_VERSION := $(shell cargo metadata --format-version 1 | jq -r '.packages[] | select(.name == "trident") | .version'))
	$(eval GIT_COMMIT := $(shell git rev-parse --short HEAD)$(shell git diff --quiet || echo '-dirty'))
	docker build --progress plain -t trident/trident-build:latest \
		--build-arg TRIDENT_VERSION="$(TRIDENT_CARGO_VERSION)-dev-$(GIT_COMMIT)" \
		--build-arg RPM_VER="$(TRIDENT_CARGO_VERSION)"\
		--build-arg RPM_REL="dev-$(GIT_COMMIT)"\
		.
	mkdir -p bin/
	id=$$(docker create trident/trident-build:latest) && \
	docker cp $$id:/work/trident.tar.gz bin/ && \
	docker rm -v $$id && \
	tar xf bin/trident.tar.gz -C bin/

.PHONY: docker-build
docker-build:
	docker build -f Dockerfile.runtime --progress plain -t trident/trident:latest .

.PHONY: clean
clean:
	cargo clean
	rm -rf bin/
	find . -name "*.profraw" -type f -delete

.PHONY: setsail-docs
setsail-docs:
	cargo build --release --package setsail --bin docbuilder --features tera,itertools
	mkdir -p target/setsail-docs
	target/release/docbuilder -o target/setsail-docs
	@echo Wrote docs to target/setsail-docs


JSON_SCHEMA_FOR_HUMANS_VER := 0.46
TRIDENT_API_SCHEMA_GENERATED := target/trident-api-docs/schema.json
TRIDENT_API_SCHEMA_CHECKED_IN := trident_api/docs/trident-api-schema.json

.PHONY: install-json-schema-for-humans
install-json-schema-for-humans:
	python3 -m pip install json-schema-for-humans==$(JSON_SCHEMA_FOR_HUMANS_VER)

target/trident-api-docs:
	mkdir -p target/trident-api-docs

.PHONY: build-api-schema
build-api-schema: target/trident-api-docs
	cargo build --release --bin schema --bin sample-hc --package trident_api --features=schema
	target/release/schema > $(TRIDENT_API_SCHEMA_GENERATED)

.PHONY: build-api-docs
build-api-docs: build-api-schema
	@if ! which generate-schema-doc; then \
		echo 'generate-schema-doc could not be found in $$PATH. Try: "make install-json-schema-for-humans"'; \
		exit 1; \
	fi

	target/release/sample-hc > trident_api/docs/sample-host-configuration.yaml
	@echo Updated sample Host Configuration in trident_api/docs/sample-host-configuration.yaml

	cp $(TRIDENT_API_SCHEMA_GENERATED) $(TRIDENT_API_SCHEMA_CHECKED_IN)
	@echo Updated $(TRIDENT_API_SCHEMA_CHECKED_IN)

	generate-schema-doc $(TRIDENT_API_SCHEMA_GENERATED) trident_api/docs/trident-api.md --config template_name=md --config with_footer=false
	@echo Wrote Markdown docs to trident_api/docs/trident-api.md

	generate-schema-doc $(TRIDENT_API_SCHEMA_GENERATED) trident_api/docs/html/trident-api.html --config with_footer=false
	@echo Wrote HTML docs to trident_api/docs/html/trident-api.html

.PHONY: validate-api-schema
validate-api-schema: build-api-schema
	@echo ""
	@echo "Validating Trident API schema..."
	@diff $(TRIDENT_API_SCHEMA_CHECKED_IN) $(TRIDENT_API_SCHEMA_GENERATED) || { \
		echo "ERROR: Trident API schema is not up to date. Please run 'make build-api-docs' and commit the changes."; \
		exit 1; \
	}
	@echo "Trident API Schema is OK!"

.PHONY: view-docs
view-docs:
	xdg-open trident_api/docs/html/trident-api.html > /dev/null 2>&1 &

.PHONY: build-functional-tests
build-functional-test:
	cargo build --tests --features functional-tests

FUNCTIONAL_TEST_DIR := /tmp/trident-test
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
functional-test: build-functional-test-cc
	cp ../k8s-tests/tools/marinerhci_test_tools/node_interface.py functional_tests/
	cp ../k8s-tests/tools/marinerhci_test_tools/ssh_node.py functional_tests/
	python3 -u -m pytest functional_tests/ --setup-show --keep-environment --test-dir $(FUNCTIONAL_TEST_DIR) --build-output $(BUILD_OUTPUT) --force-upload -vv # -k test_osutils -s

.PHONY: patch-functional-test
patch-functional-test: build-functional-test-cc
	python3 -u -m pytest functional_tests/ --setup-show --keep-environment --test-dir $(FUNCTIONAL_TEST_DIR) --build-output $(BUILD_OUTPUT) --reuse-environment -vv # -k test_osutils -s