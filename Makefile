# Path to the trident configuration file for validate and run-netlaunch
# targets.
TRIDENT_CONFIG ?= input/trident.yaml
HOST_CONFIG ?= base.yaml

.PHONY: all
all: format check test build-api-docs bin/trident-rpms.tar.gz docker-build build-functional-test coverage validate-configs generate-mermaid-diagrams

.PHONY: check
check:
	cargo check --workspace --all-features --tests
	cargo clippy --version
	cargo clippy --locked --workspace -- -D warnings 2>&1
	cargo clippy --locked --workspace --all-features -- -D warnings 2>&1
	cargo clippy --locked --workspace --tests -- -D warnings 2>&1
	cargo clippy --locked --workspace --tests --all-features -- -D warnings 2>&1
	cargo fmt -- --check

.PHONY: build
build:
	$(eval TRIDENT_CARGO_VERSION := $(shell cargo metadata --format-version 1 | jq -r '.packages[] | select(.name == "trident") | .version'))
	$(eval GIT_COMMIT := $(shell git rev-parse --short HEAD)$(shell git diff --quiet || echo '.dirty'))
	@OPENSSL_STATIC=1 OPENSSL_LIB_DIR=$(shell dirname `whereis libssl.a | cut -d" " -f2`) \
	    OPENSSL_INCLUDE_DIR=/usr/include/openssl \
	    TRIDENT_VERSION="$(TRIDENT_CARGO_VERSION)-dev.$(GIT_COMMIT)" \
	    cargo build --release --features dangerous-options
	@mkdir -p bin

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

# RPM target
bin/trident: build
	@cp -u target/release/trident bin/

bin/trident-rpms.tar.gz: Dockerfile systemd/*.service trident.spec artifacts/osmodifier bin/trident
	@docker build --quiet -t trident/trident-build:latest \
		--build-arg TRIDENT_VERSION="$(TRIDENT_CARGO_VERSION)-dev.$(GIT_COMMIT)" \
		--build-arg RPM_VER="$(TRIDENT_CARGO_VERSION)"\
		--build-arg RPM_REL="dev.$(GIT_COMMIT)"\
		.
	@mkdir -p bin/
	@id=$$(docker create trident/trident-build:latest) && \
	    docker cp -q $$id:/work/trident-rpms.tar.gz bin/ && \
	    docker rm -v $$id
	@rm -rf bin/RPMS/x86_64
	@tar xf bin/trident-rpms.tar.gz -C bin/

SYSTEMD_RPM_TAR_URL ?= https://hermesimages.blob.core.windows.net/hermes-test/systemd-254-3.tar.gz

artifacts/systemd/systemd-254-3.cm2.x86_64.rpm:
	mkdir -p ./artifacts/systemd
	curl $(SYSTEMD_RPM_TAR_URL) | tar -xz -C ./artifacts/systemd --strip-components=1
	rm -f ./artifacts/systemd/*.src.rpm ./artifacts/systemd/systemd-debuginfo*.rpm ./artifacts/systemd/systemd-devel-*.rpm

.PHONY: docker-build
docker-build: Dockerfile.runtime bin/trident-rpms.tar.gz docker-runtime-build

.PHONY: docker-runtime-build
docker-runtime-build: artifacts/systemd/systemd-254-3.cm2.x86_64.rpm
	docker build -f Dockerfile.runtime --progress plain -t trident/trident:latest .

artifacts/test-image/trident-container.bin: docker-runtime-build
	docker save trident/trident:latest > $@

.PHONY: clean
clean:
	cargo clean
	rm -rf bin/
	rm -rf artifacts/
	find . -name "*.profraw" -type f -delete

# Locally we generally want to compile in debugging mode to reuse local artifacts.
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

HC_SAMPLES = basic simple base verity advanced raid encryption
TRIDENT_API_HC_SAMPLES := docs/Reference/Host-Configuration/Samples

.PHONY: build-api-docs
build-api-docs: build-api-schema docbuilder
	$(DOCBUILDER_BIN) host-config sample -n base -m -o $(TRIDENT_API_HC_EXAMPLE_FILE)
	@echo Updated "base" sample Host Configuration in $(TRIDENT_API_HC_EXAMPLE_FILE)

	$(foreach SAMPLE_NAME,$(HC_SAMPLES),$(DOCBUILDER_BIN) host-config sample -n $(SAMPLE_NAME) -o $(TRIDENT_API_HC_SAMPLES)/$(SAMPLE_NAME).yaml &&) true

	cp $(TRIDENT_API_HC_SCHEMA_GENERATED) $(TRIDENT_API_HC_SCHEMA_CHECKED_IN)
	@echo Updated $(TRIDENT_API_HC_SCHEMA_CHECKED_IN)

	$(DOCBUILDER_BIN) host-config markdown $(TRIDENT_API_HC_MARKDOWN_DIR) --devops-wiki
	@echo Wrote Markdown docs to $(TRIDENT_API_HC_MARKDOWN_DIR)

# This target is meant to be used by CI to ensure that the API schema is up to date.
# It compares the generated schema with the checked-in schema.
# Please do not modify for local use. :)
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

PLATFORM_TESTS_DIR ?= ../platform-tests
# Allow overriding the path to the Argus toolkit
# Functional tests default to ../argus-tookit when not set
ARGUS_TOOLKIT_PATH ?=

.PHONY: functional-test
functional-test: build-functional-test-cc generate-functional-test-manifest
	cp $(PLATFORM_TESTS_DIR)/tools/marinerhci_test_tools/node_interface.py functional_tests/
	cp $(PLATFORM_TESTS_DIR)/tools/marinerhci_test_tools/ssh_node.py functional_tests/
	ARGUS_TOOLKIT_PATH=$(ARGUS_TOOLKIT_PATH) python3 -u -m pytest functional_tests/$(FILTER) -v -o junit_logging=all --junitxml $(FUNCTIONAL_TEST_JUNIT_XML) ${FUNCTIONAL_TEST_EXTRA_PARAMS} --keep-environment --test-dir $(FUNCTIONAL_TEST_DIR) --build-output $(BUILD_OUTPUT) --force-upload

.PHONY: patch-functional-test
patch-functional-test: build-functional-test-cc generate-functional-test-manifest
	ARGUS_TOOLKIT_PATH=$(ARGUS_TOOLKIT_PATH) python3 -u -m pytest functional_tests/$(FILTER) -v -o junit_logging=all --junitxml $(FUNCTIONAL_TEST_JUNIT_XML) ${FUNCTIONAL_TEST_EXTRA_PARAMS} --keep-environment --test-dir $(FUNCTIONAL_TEST_DIR) --build-output $(BUILD_OUTPUT) --reuse-environment

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

.PHONY: generate-mermaid-diagrams
generate-mermaid-diagrams: mmdc
	rm -f $(abspath dev-docs/diagrams)/*.png
	$(MAKE) $(addsuffix .png, $(basename $(wildcard $(abspath dev-docs/diagrams)/*.mmd)))

mmdc:
	docker pull ghcr.io/mermaid-js/mermaid-cli/mermaid-cli

$(abspath dev-docs/diagrams)/%.png: dev-docs/diagrams/%.mmd
	docker run --rm -u `id -u`:`id -g` -v $(abspath dev-docs/diagrams):/data minlag/mermaid-cli -i /data/$(notdir $<) -o /data/$(notdir $@)

go.sum: go.mod
	go mod tidy

bin/netlaunch: tools/cmd/netlaunch/* tools/go.sum tools/pkg/phonehome/*
	mkdir -p bin
	cd tools && go build -o ../bin/netlaunch ./cmd/netlaunch

bin/netlisten: tools/cmd/netlisten/* tools/go.sum tools/pkg/phonehome/*
	mkdir -p bin
	cd tools && go build -o ../bin/netlisten ./cmd/netlisten

.PHONY: validate
validate: $(TRIDENT_CONFIG) bin/trident
	@bin/trident validate -c $(TRIDENT_CONFIG)

.PHONY: run-netlaunch
run-netlaunch: input/netlaunch.yaml $(TRIDENT_CONFIG) bin/netlaunch bin/trident-mos.iso validate
	@mkdir -p artifacts/test-image
	@cp bin/trident artifacts/test-image
	@bin/netlaunch -i bin/trident-mos.iso -c input/netlaunch.yaml -t $(TRIDENT_CONFIG) -l -r remote-addr -s artifacts/test-image

.PHONY: run-netlaunch-container
run-netlaunch-container: input/netlaunch.yaml $(TRIDENT_CONFIG) bin/netlaunch bin/trident-containerhost-mos.iso validate artifacts/test-image/trident-container.bin
	@bin/netlaunch -i bin/trident-mos.iso -c input/netlaunch.yaml -t $(TRIDENT_CONFIG) -l -r remote-addr -s artifacts/test-image

# This target leverages the samples that are automatically generated as part of
# the build-api-docs target. The HC sample is selected by setting the
# HOST_CONFIG variable to the filename of the autogenerated sample (from
# docs/Reference/Host-Configuration/Samples). The target extends the sample
# with:
# - The current user and their SSH public key is injected into os.users.
# - Any string attribute starting with file:///trident_cdrom/data is replaced by
#   http://NETLAUNCH_HOST_ADDRESS/files.
# - The recoveryKeyUrl attribute is removed from storage.encryption (and if
#   needed will be autogenerated).
# - The sha256 attribute of each image is set to "ignored" to avoid checksum of
#   images that might be different from what the sample assumed.
# - The HC sample is wrapped in the hostConfiguration key.
# The modified sample is then used to run netlaunch.
.PHONY: run-netlaunch-sample
run-netlaunch-sample: build-api-docs
	$(eval TMP := $(shell mktemp))
	yq '.os.users += [{"name": "$(shell whoami)", "sshPublicKeys": ["$(shell cat ~/.ssh/id_rsa.pub)"], "sshMode": "key-only", "secondaryGroups": ["wheel"]}] | (.. | select(tag == "!!str")) |= sub("file:///trident_cdrom/data", "http://NETLAUNCH_HOST_ADDRESS/files") | del(.storage.encryption.recoveryKeyUrl) | .storage.images[].sha256 = "ignored" | {"hostConfiguration": .}' docs/Reference/Host-Configuration/Samples/$(HOST_CONFIG) > $(TMP)
	TRIDENT_CONFIG=$(TMP) make run-netlaunch

.PHONY: download-runtime-partition-images
download-runtime-partition-images:
	$(eval BRANCH ?= main)
	$(eval PIPELINE_IMAGES_LAST_RUN := $(shell az pipelines runs list \
		--org 'https://dev.azure.com/mariner-org' \
		--project "ECF" \
		--pipeline-ids 2195 \
		--branch $(BRANCH) \
		--query-order QueueTimeDesc \
		--result succeeded \
		--reason triggered \
		--top 1 \
		--query '[0].id'))
	@echo PIPELINE RUN ID: $(PIPELINE_IMAGES_LAST_RUN)
	$(eval DOWNLOAD_DIR := $(shell mktemp -d))
	az pipelines runs artifact download \
		--org 'https://dev.azure.com/mariner-org' \
		--project "ECF" \
		--run-id $(PIPELINE_IMAGES_LAST_RUN) \
		--path $(DOWNLOAD_DIR) \
		--artifact-name 'trident-testimg'

#   Clean & create artifacts dir
	rm -rf ./artifacts/test-image
	mkdir -p ./artifacts/test-image
#	Extract partition images. The version is dropped and the extension is changed
#	to .rawzst in case the files are inserted into the ISO filesystem
# 	instead of the initrd.
	for file in $(DOWNLOAD_DIR)/*.raw.zst; do \
		name=$$(basename $$file | cut -d'.' -f1); \
		mv $$file ./artifacts/test-image/$$name.rawzst; \
	done
#	Clean temp dir
	rm -rf $(DOWNLOAD_DIR)

# Get verity images
	$(eval DOWNLOAD_DIR := $(shell mktemp -d))
	az pipelines runs artifact download \
		--org 'https://dev.azure.com/mariner-org' \
		--project "ECF" \
		--run-id $(PIPELINE_IMAGES_LAST_RUN) \
		--path $(DOWNLOAD_DIR) \
		--artifact-name 'trident-verity-testimage'

#	Extract partition images. The version is dropped and the extension is changed
#	to .rawzst in case the files are inserted into the ISO filesystem
# 	instead of the initrd.
	for file in $(DOWNLOAD_DIR)/*.raw.zst; do \
		name=$$(basename $$file | cut -d'.' -f1); \
		mv $$file ./artifacts/test-image/verity_$$name.rawzst; \
	done
	mv ./artifacts/test-image/verity_root-hash.rawzst ./artifacts/test-image/verity_roothash.rawzst
#	Clean temp dir
	rm -rf $(DOWNLOAD_DIR)

.PHONY: copy-runtime-partition-images
copy-runtime-partition-images: ../test-images/build/trident-testimage/*.raw.zst ../test-images/build/trident-verity-testimage/*.raw.zst
# 	Check repo is adjacent
	@test -d ../test-images || { \
		echo "Test images repo not found in adjacent directory."; \
		exit 1; \
	}
#	Check directory exists
	@test -d ../test-images/build/trident-testimage || { \
		echo "Trident images not found in adjacent test-images repo."; \
		exit 1; \
	}
#	Check directory exists
	@test -d ../test-images/build/trident-verity-testimage || { \
		echo "Trident images not found in adjacent test-images repo."; \
		exit 1; \
	}
#   Clean & create artifacts dir
	@rm -rf ./artifacts/test-image
	@mkdir -p ./artifacts/test-image
	@for file in ../test-images/build/trident-testimage/*.raw.zst; do \
		name=$$(basename $$file | cut -d'.' -f1); \
		cp $$file ./artifacts/test-image/$$name.rawzst; \
		echo "Copied $$file to ./artifacts/test-image/$$name.rawzst"; \
	done
	@for file in ../test-images/build/trident-verity-testimage/*.raw.zst; do \
		name=$$(basename $$file | cut -d'.' -f1); \
		cp $$file ./artifacts/test-image/verity_$$name.rawzst; \
		echo "Copied $$file to ./artifacts/test-image/verity_$$name.rawzst"; \
	done
	mv ./artifacts/test-image/verity_root-hash.rawzst ./artifacts/test-image/verity_roothash.rawzst

BASE_IMAGE_NAME ?= baremetal_vhdx
BASE_IMAGE_VERSION ?= *
artifacts/baremetal.vhdx:
	@mkdir -p artifacts
	@tempdir=$$(mktemp -d); \
		result=$$(az artifacts universal download \
			--organization "https://dev.azure.com/mariner-org/" \
			--project "36d030d6-1d99-4ebd-878b-09af1f4f722f" \
			--scope project \
			--feed "MarinerCoreArtifacts" \
			--name '$(BASE_IMAGE_NAME)' \
			--version '$(BASE_IMAGE_VERSION)' \
			--path $$tempdir) && \
		mv $$tempdir/*.vhdx artifacts/baremetal.vhdx && \
		rm -rf $$tempdir && \
		echo $$result | jq > artifacts/baremetal.vhdx.metadata.json

MIC_PACKAGE_NAME ?= imagecustomizer_preview
MIC_PACKAGE_VERSION ?= 0.1.0-preview.525339
artifacts/imagecustomizer:
	@mkdir -p artifacts
	@az artifacts universal download \
	    --organization "https://dev.azure.com/mariner-org/" \
	    --project "36d030d6-1d99-4ebd-878b-09af1f4f722f" \
	    --scope project \
	    --feed "MarinerCoreArtifacts" \
	    --name '$(MIC_PACKAGE_NAME)' \
	    --version '$(MIC_PACKAGE_VERSION)' \
	    --path artifacts/
	@chmod +x artifacts/imagecustomizer
	@touch artifacts/imagecustomizer

bin/trident-mos.iso: artifacts/baremetal.vhdx artifacts/imagecustomizer trident-mos/iso.yaml artifacts/systemd/systemd-254-3.cm2.x86_64.rpm trident-mos/files/* trident-mos/post-install.sh
	BUILD_DIR=`mktemp -d`; \
		sudo ./artifacts/imagecustomizer \
			--log-level=debug \
			--rpm-source ./artifacts/systemd \
			--build-dir $$BUILD_DIR \
			--image-file $< \
			--output-image-file $@ \
			--config-file trident-mos/iso.yaml \
			--output-image-format iso; \
		sudo rm -rf $$BUILD_DIR
	sudo rm -r artifacts/systemd/repodata
bin/trident-containerhost-mos.iso: artifacts/baremetal.vhdx artifacts/imagecustomizer trident-mos/containerhost-iso.yaml trident-mos/files/* trident-mos/post-install.sh
	BUILD_DIR=`mktemp -d`; \
		sudo ./artifacts/imagecustomizer \
			--log-level=debug \
			--build-dir $$BUILD_DIR \
			--image-file $< \
			--output-image-file $@ \
			--config-file trident-mos/containerhost-iso.yaml \
			--output-image-format iso; \
		sudo rm -rf $$BUILD_DIR
