# Path to the trident configuration file for validate and run-netlaunch
# targets.
TRIDENT_CONFIG ?= input/trident.yaml

ARGUS_TOOLKIT_PATH ?= ../argus-toolkit

PLATFORM_TESTS_PATH ?= ../platform-tests

TEST_IMAGES_PATH ?= ../test-images

HOST_CONFIG ?= base.yaml

NETLAUNCH_CONFIG ?= input/netlaunch.yaml

.PHONY: all
all: format check test build-api-docs bin/trident-rpms-azl2.tar.gz bin/trident-rpms-azl3.tar.gz docker-build build-functional-test coverage validate-configs generate-mermaid-diagrams

.PHONY: check
check:
	cargo fmt -- --check
	cargo check --workspace --all-features --tests
	cargo clippy --version
	cargo clippy --locked --workspace -- -D warnings 2>&1
	cargo clippy --locked --workspace --all-features -- -D warnings 2>&1
	cargo clippy --locked --workspace --tests -- -D warnings 2>&1
	cargo clippy --locked --workspace --tests --all-features -- -D warnings 2>&1

.PHONY: check-pipelines
check-pipelines:
ifdef BRANCH
	$(eval BRANCH_FLAG := -b $(BRANCH))
endif
	./scripts/test-pipeline pr -q $(BRANCH_FLAG)
	./scripts/test-pipeline pr-e2e -q $(BRANCH_FLAG)
	./scripts/test-pipeline ci -q $(BRANCH_FLAG)
	./scripts/test-pipeline pre -q $(BRANCH_FLAG)
	./scripts/test-pipeline rel -q $(BRANCH_FLAG)
	./scripts/test-pipeline testing -q $(BRANCH_FLAG)
	./scripts/test-pipeline tester -q $(BRANCH_FLAG)

# Local override of the cargo config to avoid having to go through the registry
.cargo/config: .cargo/config.toml
	@cp $< $@
	@sed -i 's|replace-with = "BMP_PublicPackages"|# &|' $@
	@echo "NOTICE: Created local .cargo/config file."

.PHONY: build
build: .cargo/config
	$(eval TRIDENT_CARGO_VERSION := $(shell cargo metadata --format-version 1 | jq -r '.packages[] | select(.name == "trident") | .version'))
	$(eval GIT_COMMIT := $(shell git rev-parse --short HEAD)$(shell git diff --quiet || echo '.dirty'))
	@OPENSSL_STATIC=1 \
		OPENSSL_LIB_DIR=$(shell dirname `whereis libssl.a | cut -d" " -f2`) \
		OPENSSL_INCLUDE_DIR=/usr/include/openssl \
		TRIDENT_VERSION="$(TRIDENT_CARGO_VERSION)-dev.$(GIT_COMMIT)" \
		cargo build --release --features dangerous-options
	@mkdir -p bin

.PHONY: format
format:
	cargo fmt
	python3 -m black .

.PHONY: test
test: .cargo/config
	cargo test --all --no-fail-fast

.PHONY: ut-coverage
ut-coverage: .cargo/config
	mkdir -p target/coverage/profraw
	CARGO_INCREMENTAL=0 RUSTFLAGS='-Cinstrument-coverage' LLVM_PROFILE_FILE='target/coverage/profraw/cargo-test-%p-%m.profraw' cargo test --target-dir target/coverage --all --no-fail-fast

.PHONY: coverage-report
coverage-report: .cargo/config
	# cargo install grcov
	grcov . --binary-path ./target/coverage/debug/deps/ -s . -t html,covdir,cobertura --branch --ignore-not-existing --ignore '../*' --ignore "/*" --ignore "docbuilder/*" --ignore "target/*" -o target/coverage
	jq .coveragePercent target/coverage/covdir

.PHONY: coverage
coverage: ut-coverage coverage-report

.PHONY: clean-coverage
clean-coverage:
	rm -rf target/coverage/profraw

EMU_PACKAGE_NAME ?= osmodifier_preview

.PHONY: print-var-emu-package-name
print-var-emu-package-name:
	@echo $(EMU_PACKAGE_NAME)

EMU_PACKAGE_VERSION ?= 0.7.0-preview.650313

.PHONY: print-var-emu-package-version
print-var-emu-package-version:
	@echo $(EMU_PACKAGE_VERSION)

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

bin/trident: build
	@mkdir -p bin
	@cp -u target/release/trident bin/

bin/trident-rpms-azl2.tar.gz: Dockerfile.azl2 systemd/*.service trident.spec artifacts/osmodifier bin/trident
	@docker build --quiet -t trident/trident-build:latest \
		--build-arg TRIDENT_VERSION="$(TRIDENT_CARGO_VERSION)-dev.$(GIT_COMMIT)" \
		--build-arg RPM_VER="$(TRIDENT_CARGO_VERSION)" \
		--build-arg RPM_REL="dev.$(GIT_COMMIT)" \
		-f Dockerfile.azl2 \
		.
	@id=$$(docker create trident/trident-build:latest) && \
	    docker cp -q $$id:/work/trident-rpms.tar.gz $@ && \
	    docker rm -v $$id
	@rm -rf bin/RPMS/x86_64
	@tar xf $@ -C bin/

bin/trident-rpms-azl3.tar.gz: Dockerfile.azl3 systemd/*.service trident.spec artifacts/osmodifier bin/trident
	@docker build -t trident/trident-build:latest \
		--build-arg TRIDENT_VERSION="$(TRIDENT_CARGO_VERSION)-dev.$(GIT_COMMIT)" \
		--build-arg RPM_VER="$(TRIDENT_CARGO_VERSION)" \
		--build-arg RPM_REL="dev.$(GIT_COMMIT)" \
		-f Dockerfile.azl3 \
		.
	@mkdir -p bin/
	@id=$$(docker create trident/trident-build:latest) && \
	    docker cp -q $$id:/work/trident-rpms.tar.gz $@ || \
	    docker rm -v $$id
	@rm -rf bin/RPMS/x86_64
	@tar xf $@ -C bin/

bin/trident-rpms.tar.gz: bin/trident-rpms-azl3.tar.gz
	cp $< $@

.PHONY: docker-build
docker-build: Dockerfile.runtime bin/trident-rpms-azl3.tar.gz
	@docker build --quiet -f Dockerfile.runtime -t trident/trident:latest .

artifacts/test-image/trident-container.tar.gz: docker-build
	@mkdir -p artifacts/test-image
	@CONTAINER_ID=$$(docker inspect --format='{{index .Id}}' trident/trident:latest); \
	if [ ! -f $@ ] || [ ! -f bin/container-id ] || [ $CONTAINER_ID != "$$(cat bin/container-id)" ]; then \
		docker save trident/trident:latest | zstd > $@ && \
		echo $CONTAINER_ID > bin/container-id; \
	fi

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
docbuilder: .cargo/config
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
TRIDENT_API_HC_EXAMPLE_FILE := docs/Reference/Host-Configuration/Sample-Host-Configuration.md
TRIDENT_API_HC_EXAMPLE_YAML := docs/Reference/Host-Configuration/sample-host-configuration.yaml
TRIDENT_API_HC_STORAGE_RULES_FILES := docs/Reference/Host-Configuration/Storage-Rules.md

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
	$(DOCBUILDER_BIN) host-config sample -n base -o $(TRIDENT_API_HC_EXAMPLE_YAML)
	@echo Updated "base" sample Host Configuration in $(TRIDENT_API_HC_EXAMPLE_FILE) and $(TRIDENT_API_HC_EXAMPLE_YAML)

	$(foreach SAMPLE_NAME,$(HC_SAMPLES),$(DOCBUILDER_BIN) host-config sample -n $(SAMPLE_NAME) -o $(TRIDENT_API_HC_SAMPLES)/$(SAMPLE_NAME).yaml &&) true

	cp $(TRIDENT_API_HC_SCHEMA_GENERATED) $(TRIDENT_API_HC_SCHEMA_CHECKED_IN)
	@echo Updated $(TRIDENT_API_HC_SCHEMA_CHECKED_IN)

	$(DOCBUILDER_BIN) host-config markdown $(TRIDENT_API_HC_MARKDOWN_DIR) --devops-wiki
	@echo Wrote Markdown docs to $(TRIDENT_API_HC_MARKDOWN_DIR)

	$(DOCBUILDER_BIN) host-config storage-rules -o $(TRIDENT_API_HC_STORAGE_RULES_FILES)
	@echo Wrote storage rules to $(TRIDENT_API_HC_STORAGE_RULES_FILES)

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
build-functional-test: .cargo/config
	cargo build --tests --features functional-test --all

FUNCTIONAL_TEST_DIR := /tmp/trident-test
FUNCTIONAL_TEST_JUNIT_XML := target/trident_functional_tests.xml
TRIDENT_COVERAGE_TARGET := target/coverage
BUILD_OUTPUT := $(shell mktemp)

.PHONY: build-functional-tests-cc
build-functional-test-cc: .cargo/config
	# Redirect output to get to the test binaries; needs to be in sync with below
	-@OPENSSL_STATIC=1 \
		OPENSSL_LIB_DIR=$(shell dirname `whereis libssl.a | cut -d" " -f2`) \
		OPENSSL_INCLUDE_DIR=/usr/include/openssl \
		CARGO_INCREMENTAL=0 \
		RUSTFLAGS='-Cinstrument-coverage' \
		LLVM_PROFILE_FILE='target/coverage/profraw/cargo-test-%p-%m.profraw' \
		cargo build --target-dir $(TRIDENT_COVERAGE_TARGET) --lib --tests --features functional-test --all --message-format=json > $(BUILD_OUTPUT)

	# Output this in case there were build failures
	@OPENSSL_STATIC=1 \
		OPENSSL_LIB_DIR=$(shell dirname `whereis libssl.a | cut -d" " -f2`) \
		OPENSSL_INCLUDE_DIR=/usr/include/openssl \
		CARGO_INCREMENTAL=0 \
		RUSTFLAGS='-Cinstrument-coverage' \
		LLVM_PROFILE_FILE='target/coverage/profraw/cargo-test-%p-%m.profraw' \
		cargo build --target-dir $(TRIDENT_COVERAGE_TARGET) --lib --tests --features functional-test --all

.PHONY: functional-test
functional-test: bin/trident-mos.iso bin/trident artifacts/osmodifier artifacts/test-image/*.rawzst bin/netlaunch
	cp $(PLATFORM_TESTS_PATH)/tools/marinerhci_test_tools/node_interface.py functional_tests/
	cp $(PLATFORM_TESTS_PATH)/tools/marinerhci_test_tools/ssh_node.py functional_tests/
	cp bin/trident artifacts/test-image/
	cp artifacts/osmodifier artifacts/test-image/
	$(MAKE) functional-test-core

# A target for pipelines that skips all setup and building steps that are not
# required in the pipeline environment.
.PHONY: functional-test-core
functional-test-core: build-functional-test-cc generate-functional-test-manifest
# Check if INSTALLER_ISO_PATH is set, if not, check if the installer iso is present in the bin directory
ifndef INSTALLER_ISO_PATH
ifeq ($(wildcard bin/trident-mos.iso),)
	$(error INSTALLER_ISO_PATH is not set and bin/trident-mos.iso is not present in the bin directory)
endif
endif
	python3 -u -m pytest functional_tests/test_setup.py functional_tests/$(FILTER) --keep-duplicates -v -o junit_logging=all --junitxml $(FUNCTIONAL_TEST_JUNIT_XML) ${FUNCTIONAL_TEST_EXTRA_PARAMS} --keep-environment --test-dir $(FUNCTIONAL_TEST_DIR) --build-output $(BUILD_OUTPUT) --force-upload

.PHONY: patch-functional-test
patch-functional-test: build-functional-test-cc generate-functional-test-manifest
	ARGUS_TOOLKIT_PATH=$(ARGUS_TOOLKIT_PATH) python3 -u -m pytest functional_tests/$(FILTER) -v -o junit_logging=all --junitxml $(FUNCTIONAL_TEST_JUNIT_XML) ${FUNCTIONAL_TEST_EXTRA_PARAMS} --keep-environment --test-dir $(FUNCTIONAL_TEST_DIR) --build-output $(BUILD_OUTPUT) --reuse-environment

.PHONY: generate-functional-test-manifest
generate-functional-test-manifest: .cargo/config
	cargo build --features=pytest-generator,functional-test
	target/debug/trident pytest

.PHONY: validate-configs
validate-configs: bin/trident
	$(eval DETECTED_HC_FILES := $(shell grep -R 'hostConfiguration:' . --include '*.yaml' --exclude-dir=target --exclude-dir=dev -l))
	@for file in $(DETECTED_HC_FILES); do \
		echo "Validating $$file"; \
		$< validate -c $$file; \
	done

.PHONY: generate-mermaid-diagrams
generate-mermaid-diagrams: mmdc
	$(MAKE) $(addsuffix .png, $(basename $(wildcard $(abspath dev-docs/diagrams)/*.mmd)))
	$(MAKE) $(addsuffix .png, $(basename $(wildcard $(abspath dev-docs/diagrams/state-diagrams)/*.mmd)))

mmdc:
	docker pull ghcr.io/mermaid-js/mermaid-cli/mermaid-cli

$(abspath dev-docs/diagrams)/%.png: dev-docs/diagrams/%.mmd
	docker run --rm -u `id -u`:`id -g` -v $(abspath dev-docs/diagrams):/data minlag/mermaid-cli -i /data/$(notdir $<) -o /data/$(notdir $@)

$(abspath dev-docs/diagrams/state-diagrams)/%.png: dev-docs/diagrams/state-diagrams/%.mmd
	docker run --rm -u `id -u`:`id -g` -v $(abspath dev-docs/diagrams/state-diagrams):/data minlag/mermaid-cli -i /data/$(notdir $<) -o /data/$(notdir $@)

go.sum: go.mod
	go mod tidy

.PHONY: go-tools
go-tools: bin/netlaunch bin/netlisten bin/miniproxy

bin/netlaunch: tools/cmd/netlaunch/* tools/go.sum tools/pkg/phonehome/*
	@mkdir -p bin
	cd tools && go build -o ../bin/netlaunch ./cmd/netlaunch

bin/netlisten: tools/cmd/netlisten/* tools/go.sum tools/pkg/phonehome/*
	@mkdir -p bin
	cd tools && go build -o ../bin/netlisten ./cmd/netlisten

bin/miniproxy: tools/cmd/miniproxy/* tools/go.sum
	mkdir -p bin
	cd tools && go build -o ../bin/miniproxy ./cmd/miniproxy

bin/mkcosi: tools/cmd/mkcosi/* tools/go.sum tools/pkg/* tools/cmd/mkcosi/variants/*
	@mkdir -p bin
	cd tools && go build -o ../bin/mkcosi ./cmd/mkcosi

.PHONY: validate
validate: $(TRIDENT_CONFIG) bin/trident
	@bin/trident validate -c $(TRIDENT_CONFIG)

NETLAUNCH_ISO ?= bin/trident-mos.iso

input/netlaunch.yaml: $(ARGUS_TOOLKIT_PATH)/vm-netlaunch.yaml
	@mkdir -p input
	ln -vsf "$$(realpath "$<")" $@

.PHONY: run-netlaunch
run-netlaunch: $(NETLAUNCH_CONFIG) $(TRIDENT_CONFIG) $(NETLAUNCH_ISO) bin/netlaunch validate artifacts/osmodifier
	@mkdir -p artifacts/test-image
	@cp bin/trident artifacts/test-image/
	@cp artifacts/osmodifier artifacts/test-image/
	@bin/netlaunch \
	 	--iso $(NETLAUNCH_ISO) \
		$(if $(NETLAUNCH_PORT),--port $(NETLAUNCH_PORT)) \
		--config $(NETLAUNCH_CONFIG) \
		--trident $(TRIDENT_CONFIG) \
		--logstream \
		--remoteaddress remote-addr \
		--servefolder artifacts/test-image \
		--trace-file trident-metrics.jsonl \
		$(if $(LOG_TRACE),--log-trace)


#  To run this VM requires at least 11 GiB of memory (virt-deploy create --mem 11).
.PHONY: run-netlaunch-container-images
run-netlaunch-container-images: \
	validate \
	$(NETLAUNCH_CONFIG) \
	artifacts/trident-container-installer-testimage.iso \
	artifacts/test-image/trident-container.tar.gz \
	$(TRIDENT_CONFIG) \
	bin/netlaunch
	@bin/netlaunch \
		--iso artifacts/trident-container-installer-testimage.iso \
		$(if $(NETLAUNCH_PORT),--port $(NETLAUNCH_PORT)) \
		--config $(NETLAUNCH_CONFIG) \
		--trident $(TRIDENT_CONFIG) \
		--logstream \
		--remoteaddress remote-addr \
		--servefolder artifacts/test-image \
		--trace-file trident-metrics.jsonl \
		$(if $(LOG_TRACE),--log-trace)

.PHONY: watch-virtdeploy
watch-virtdeploy:
	@while true; do virsh console virtdeploy-vm-0; sleep 1; done

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
	yq '.os.users += [{"name": "$(shell whoami)", "sshPublicKeys": ["$(shell cat ~/.ssh/id_rsa.pub)"], "sshMode": "key-only", "secondaryGroups": ["wheel"]}] | (.. | select(tag == "!!str")) |= sub("file:///trident_cdrom/data", "http://NETLAUNCH_HOST_ADDRESS/files") | del(.storage.encryption.recoveryKeyUrl) | (.storage.filesystems[] | select(has("source")) | .source).sha256 = "ignored" | .storage.verityFilesystems[].dataImage.sha256 = "ignored" | .storage.verityFilesystems[].hashImage.sha256 = "ignored" | {"hostConfiguration": .}' docs/Reference/Host-Configuration/Samples/$(HOST_CONFIG) > $(TMP)
	TRIDENT_CONFIG=$(TMP) make run-netlaunch

.PHONY: download-runtime-partition-images
download-runtime-partition-images:
#	TODO: When bug 9348 is fixed, update the query to select only the latest
#	successful trident-ci pipeline run (--result succeeded).
	$(eval BRANCH ?= main)
	$(eval RUN_ID ?= $(shell az pipelines runs list \
		--org "https://dev.azure.com/mariner-org" \
		--project "ECF" \
		--pipeline-ids 3371 \
		--branch $(BRANCH) \
		--query-order QueueTimeDesc \
		--reason triggered \
		--top 10 \
		| jq -r '[.[] \
			| select(.result == "succeeded" or .result == "partiallySucceeded") \
			| .id] | first'))
	@echo PIPELINE RUN ID: $(RUN_ID)
	$(eval DOWNLOAD_DIR := $(shell mktemp -d))
	az pipelines runs artifact download \
		--org 'https://dev.azure.com/mariner-org' \
		--project "ECF" \
		--run-id $(RUN_ID) \
		--path $(DOWNLOAD_DIR) \
		--artifact-name 'trident-testimage'

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
		--run-id $(RUN_ID) \
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

# Get container images
	$(eval DOWNLOAD_DIR := $(shell mktemp -d))
	az pipelines runs artifact download \
		--org 'https://dev.azure.com/mariner-org' \
		--project "ECF" \
		--run-id $(RUN_ID) \
		--path $(DOWNLOAD_DIR) \
		--artifact-name 'trident-container-testimage'

#	Extract partition images. The version is dropped and the extension is changed
#	to .rawzst in case the files are inserted into the ISO filesystem
# 	instead of the initrd.
	for file in $(DOWNLOAD_DIR)/*.raw.zst; do \
		name=$$(basename $$file | cut -d'.' -f1); \
		mv $$file ./artifacts/test-image/container_$$name.rawzst; \
	done
#	Clean temp dir
	rm -rf $(DOWNLOAD_DIR)

.PHONY: download-trident-installer-iso
download-trident-installer-iso:
ifndef RUN_ID
	$(error RUN_ID is not set)
endif
	mkdir -p ./artifacts
	az pipelines runs artifact download \
		--org 'https://dev.azure.com/mariner-org' \
		--project "ECF" \
		--run-id $(RUN_ID) \
		--path artifacts/ \
		--artifact-name 'trident-installer-testimage'

.PHONY: download-trident-container-installer-iso
download-trident-container-installer-iso:
#	TODO: When bug 9348 is fixed, update the query to select only the latest
#	successful trident-ci pipeline run (--result succeeded).
	$(eval BRANCH ?= main)
	$(eval RUN_ID ?= $(shell az pipelines runs list \
		--org "https://dev.azure.com/mariner-org" \
		--project "ECF" \
		--pipeline-ids 3371 \
		--branch $(BRANCH) \
		--query-order QueueTimeDesc \
		--reason triggered \
		--top 10 \
		| jq -r '[.[] \
			| select(.result == "succeeded" or .result == "partiallySucceeded") \
			| .id] | first'))
	@echo PIPELINE RUN ID: $(RUN_ID)
	mkdir -p ./artifacts
	az pipelines runs artifact download \
		--org 'https://dev.azure.com/mariner-org' \
		--project "ECF" \
		--run-id $(RUN_ID) \
		--path artifacts/ \
		--artifact-name 'trident-container-installer-testimage'

artifacts/trident-container-installer-testimage.iso:
	$(MAKE) download-trident-container-installer-iso; \
	ls -l artifacts/trident-container-installer-testimage.iso

.PHONY: copy-runtime-partition-images
copy-runtime-partition-images: $(TEST_IMAGES_PATH)/build/trident-testimage/*.raw.zst $(TEST_IMAGES_PATH)/build/trident-verity-testimage/*.raw.zst
	@test -d $(TEST_IMAGES_PATH) || { \
		echo "$(TEST_IMAGES_PATH) not found"; \
		exit 1; \
	}
	@test -d $(TEST_IMAGES_PATH)/build/trident-testimage || { \
		echo "$(TEST_IMAGES_PATH)/build/trident-testimage not found"; \
		exit 1; \
	}
	@test -d $(TEST_IMAGES_PATH)/build/trident-verity-testimage || { \
		echo "$(TEST_IMAGES_PATH)/build/trident-verity-testimage not found"; \
		exit 1; \
	}
	@rm -rf ./artifacts/test-image
	@mkdir -p ./artifacts/test-image
	@for file in $(TEST_IMAGES_PATH)/build/trident-testimage/*.raw.zst; do \
		name=$$(basename $$file | cut -d'.' -f1); \
		cp $$file ./artifacts/test-image/$$name.rawzst; \
		echo "Copied $$file to ./artifacts/test-image/$$name.rawzst"; \
	done
	@for file in $(TEST_IMAGES_PATH)/build/trident-verity-testimage/*.raw.zst; do \
		name=$$(basename $$file | cut -d'.' -f1); \
		cp $$file ./artifacts/test-image/verity_$$name.rawzst; \
		echo "Copied $$file to ./artifacts/test-image/verity_$$name.rawzst"; \
	done
	mv ./artifacts/test-image/verity_root-hash.rawzst ./artifacts/test-image/verity_roothash.rawzst
	@for file in $(TEST_IMAGES_PATH)/build/trident-container-testimage/*.raw.zst; do \
		name=$$(basename $$file | cut -d'.' -f1); \
		cp $$file ./artifacts/test-image/container_$$name.rawzst; \
		echo "Copied $$file to ./artifacts/test-image/container_$$name.rawzst"; \
	done

# Uses the simple e2e test to set up a starter host config
.PHONY: starter-configuration
starter-configuration:
	@mkdir -p $$(dirname $(TRIDENT_CONFIG))
	@cp e2e_tests/trident_configurations/simple/trident-config.yaml $(TRIDENT_CONFIG)
	@echo "\033[33mCreated \033[36m$(TRIDENT_CONFIG)\033[33m. Please review and modify as needed! :)"
	@echo "\033[33mDon't forget to add your SSH public key to the host configuration!"

BASE_IMAGE_NAME ?= baremetal_vhdx-3.0-stable
BASE_IMAGE_VERSION ?= *
artifacts/baremetal.vhdx:
	@mkdir -p artifacts
	@tempdir=$$(mktemp -d); \
		result=$$(az artifacts universal download \
			--organization "https://dev.azure.com/mariner-org/" \
			--project "36d030d6-1d99-4ebd-878b-09af1f4f722f" \
			--scope project \
			--feed "AzureLinuxArtifacts" \
			--name '$(BASE_IMAGE_NAME)' \
			--version '$(BASE_IMAGE_VERSION)' \
			--path $$tempdir) && \
		mv $$tempdir/*.vhdx artifacts/baremetal.vhdx && \
		rm -rf $$tempdir && \
		echo $$result | jq > artifacts/baremetal.vhdx.metadata.json

MIC_PACKAGE_NAME ?= imagecustomizer
MIC_PACKAGE_VERSION ?= *
artifacts/imagecustomizer:
	@mkdir -p artifacts
	@az artifacts universal download \
	    --organization "https://dev.azure.com/mariner-org/" \
	    --project "36d030d6-1d99-4ebd-878b-09af1f4f722f" \
	    --scope project \
	    --feed "AzureLinuxArtifacts" \
	    --name '$(MIC_PACKAGE_NAME)' \
	    --version '$(MIC_PACKAGE_VERSION)' \
	    --path artifacts/
	@chmod +x artifacts/imagecustomizer
	@touch artifacts/imagecustomizer

bin/trident-mos.iso: artifacts/baremetal.vhdx artifacts/imagecustomizer trident-mos/iso.yaml trident-mos/files/* trident-mos/post-install.sh
	@mkdir -p bin
	BUILD_DIR=`mktemp -d` && \
		trap 'sudo rm -rf $$BUILD_DIR' EXIT; \
		sudo ./artifacts/imagecustomizer \
			--log-level=debug \
			--build-dir $$BUILD_DIR \
			--image-file $< \
			--output-image-file $@ \
			--config-file trident-mos/iso.yaml \
			--output-image-format iso
