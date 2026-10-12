CARGO_TAILOR ?= $(if $(shell command -v cargo 2>/dev/null),cargo,$(HOME)/.cargo/bin/cargo)
TAILOR_DIR := $(patsubst %/,%,$(dir $(lastword $(MAKEFILE_LIST))))
TAILOR_WORKSPACE := $(TAILOR_DIR)/tailor.yaml

# Matches tailor.yaml's default `ic` toolchain entry. Also used directly by
# bin/trident-mos.iso (see root Makefile). Override to build against a dev
# imagecustomizer; only overridden values trigger the toolchain swap below.
TAILOR_DEFAULT_IC_IMAGE := mcr.microsoft.com/azurelinux/imagecustomizer:latest
MIC_CONTAINER_IMAGE ?= $(TAILOR_DEFAULT_IC_IMAGE)
TAILOR_RPM_DEPS := bin/RPMS $(shell find bin/RPMS -name '*.rpm' 2>/dev/null) artifacts/rpm-overrides
VM_IMAGE_PATH_PREFIX := $(TAILOR_DIR)/trident-vm-testimage/base

# Shared files under common/ are referenced (via extraDependencies) by every
# family below; depend on them everywhere so edits there trigger a rebuild.
TAILOR_COMMON_DEPS := $(shell find $(TAILOR_DIR)/common -type f 2>/dev/null)

TRIDENT_INSTALLER_DEPS := $(shell find $(TAILOR_DIR)/trident-installer -type f 2>/dev/null) $(TAILOR_COMMON_DEPS)
TRIDENT_TESTIMAGE_DEPS := $(shell find $(TAILOR_DIR)/trident-testimage -type f 2>/dev/null) $(TAILOR_COMMON_DEPS)
TRIDENT_VERITY_TESTIMAGE_DEPS := $(shell find $(TAILOR_DIR)/trident-verity-testimage -type f 2>/dev/null) $(TAILOR_COMMON_DEPS)
TRIDENT_CONTAINER_INSTALLER_DEPS := $(shell find $(TAILOR_DIR)/trident-container-installer -type f 2>/dev/null) $(TAILOR_COMMON_DEPS)
TRIDENT_CONTAINER_TESTIMAGE_DEPS := $(shell find $(TAILOR_DIR)/trident-container-testimage -type f 2>/dev/null) $(TAILOR_COMMON_DEPS)
TRIDENT_FUNCTEST_DEPS := $(shell find $(TAILOR_DIR)/trident-functest -type f 2>/dev/null)
AZL_INSTALLER_TAILOR_DEPS := $(shell find $(AZL_INSTALLER_DIR) -type f 2>/dev/null)
AZURELINUX_DIRECT_STREAMING_DEPS := $(shell find $(TAILOR_DIR)/azurelinux-direct-streaming-testimage -type f 2>/dev/null) $(TAILOR_COMMON_DEPS)
FOREIGN_DIRECT_STREAMING_DEPS := $(shell find $(TAILOR_DIR)/foreign-direct-streaming-testimage -type f 2>/dev/null)
TRIDENT_VM_TESTIMAGE_DEPS := $(shell find $(TAILOR_DIR)/trident-vm-testimage -type f 2>/dev/null) $(VM_IMAGE_PATH_PREFIX)/files/id_rsa.pub $(TAILOR_COMMON_DEPS)

COSI_TARGETS = \
	trident-testimage \
	trident-testimage-arm64 \
	trident-verity-testimage \
	trident-usrverity-testimage \
	trident-container-verity-testimage \
	trident-container-usrverity-testimage \
	trident-container-testimage \
	azurelinux-direct-streaming-testimage-amd64 \
	azurelinux-direct-streaming-testimage-arm64 \
	trident-vm-grub-testimage \
	trident-vm-grub-verity-testimage \
	trident-vm-root-verity-testimage \
	trident-vm-usr-verity-testimage \
	trident-vm-acl-agent-update-testimage \
	trident-vm-grub-verity-azure-testimage \
	trident-vm-grub-testimage-arm64 \
	trident-vm-grub-verity-testimage-arm64 \
	ubuntu-direct-streaming-testimage-2204-amd64 \
	ubuntu-direct-streaming-testimage-2204-arm64 \
	ubuntu-direct-streaming-testimage-2404-amd64 \
	ubuntu-direct-streaming-testimage-2404-arm64 \
	gb200-direct-streaming-testimage-2404-arm64

ISO_TARGETS = \
	trident-installer \
	trident-split-installer \
	trident-installer-arm64 \
	trident-container-installer \
	trident-direct-streaming-installer-amd64 \
	trident-direct-streaming-installer-arm64 \
	azl-installer

.PHONY: $(COSI_TARGETS) $(ISO_TARGETS) all-cosi all-iso
$(COSI_TARGETS): %: artifacts/%.cosi
$(filter-out azl-installer,$(ISO_TARGETS)): %: artifacts/%.iso
azl-installer: $(ARTIFACTS_TEST_IMAGE_DIR)/azl-installer.iso

all-cosi: $(COSI_TARGETS)
all-iso: $(ISO_TARGETS)

# Per-target, not shared: avoids `make -j` races where two in-flight builds
# would otherwise stomp on the same staging path (row 20).
define tailor_ca_cert_staging_path
$(TAILOR_DIR)/artifacts/ca_cert.$(subst /,_,$1).pem
endef

define declare_tailor_target
$1: $(TAILOR_WORKSPACE) $4
	@echo "Building '$$(@F)' [$@]"
	@echo "Prerequisites:"
	@echo "$$^" | tr ' ' '\n' | sed 's/^/    /'
	rm -f $(call tailor_ca_cert_staging_path,$1)
	@manifest="$(TAILOR_DIR)/.tailor.local.$(subst /,_,$1).yaml"; \
	sed_args=(-e 's@^      publishCaCert: \./artifacts/ca_cert\.pem$$$$@      publishCaCert: ./artifacts/ca_cert.$(subst /,_,$1).pem@'); \
	if [ "$(MIC_CONTAINER_IMAGE)" != "$(TAILOR_DEFAULT_IC_IMAGE)" ]; then \
		mic="$(MIC_CONTAINER_IMAGE)"; image="$$$${mic%:*}"; tag="$$$${mic##*:}"; \
		if [ "$$$$image" = "$$$$tag" ]; then image="$$$$mic"; tag="latest"; fi; \
		sed_args+=(-e "s/^  default: ic$$$$/  default: ic-dev/" \
		           -e "s/^      tag: latest$$$$/      tag: latest\n    - name: ic-dev\n      container: $$$$image\n      tag: $$$$tag\n      pull: never/"); \
	fi; \
	sed "$$$${sed_args[@]}" "$(TAILOR_WORKSPACE)" > "$$$$manifest"; \
	$(CARGO_TAILOR) run --manifest-path tools/tailor/Cargo.toml --quiet -- -vvv --log-dir $(TAILOR_DIR)/artifacts/tailor-logs --manifest "$$$$manifest" build $2 $3 --output-dir $(dir $1)
	@rm -f $(TAILOR_DIR)/.tailor.local.$(subst /,_,$1).yaml
	@if [ -f $(call tailor_ca_cert_staging_path,$1) ]; then cp $(call tailor_ca_cert_staging_path,$1) "$(dir $1)/ca_cert.pem"; fi
	@if [ "$1" != "$5" ]; then cp "$5" "$1"; fi
	@$6
endef

$(eval $(call declare_tailor_target,artifacts/trident-installer.iso,trident-installer,-s arch=amd64 -s variant=default,$(TRIDENT_INSTALLER_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx,artifacts/trident-installer.iso,:))
$(eval $(call declare_tailor_target,artifacts/trident-split-installer.iso,trident-installer,-s arch=amd64 -s variant=split,$(TRIDENT_INSTALLER_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx,artifacts/trident-split-installer.iso,:))
$(eval $(call declare_tailor_target,artifacts/trident-installer-arm64.iso,trident-installer,-s arch=arm64 -s variant=default,$(TRIDENT_INSTALLER_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_arm64.vhdx,artifacts/trident-installer-arm64.iso,:))
$(eval $(call declare_tailor_target,artifacts/trident-direct-streaming-installer-amd64.iso,trident-installer,-s arch=amd64 -s variant=direct-streaming,$(TRIDENT_INSTALLER_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx artifacts/rcp-agent artifacts/rcp-agent.service,artifacts/trident-direct-streaming-installer-amd64.iso,:))
$(eval $(call declare_tailor_target,artifacts/trident-direct-streaming-installer-arm64.iso,trident-installer,-s arch=arm64 -s variant=direct-streaming,$(TRIDENT_INSTALLER_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_arm64.vhdx artifacts/rcp-agent artifacts/rcp-agent.service,artifacts/trident-direct-streaming-installer-arm64.iso,:))
$(eval $(call declare_tailor_target,artifacts/trident-container-installer.iso,trident-container-installer,,$(TRIDENT_CONTAINER_INSTALLER_DEPS) artifacts/baremetal.vhdx,artifacts/trident-container-installer.iso,:))
$(eval $(call declare_tailor_target,artifacts/trident-functest.qcow2,trident-functest,,$(TRIDENT_FUNCTEST_DEPS) artifacts/baremetal.vhdx,artifacts/trident-functest.qcow2,:))
$(eval $(call declare_tailor_target,artifacts/trident-testimage.cosi,trident-testimage,-s arch=amd64,$(TRIDENT_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx,artifacts/trident-testimage.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-testimage-arm64.cosi,trident-testimage,-s arch=arm64,$(TRIDENT_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_arm64.vhdx,artifacts/trident-testimage-arm64.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-verity-testimage.cosi,trident-verity-testimage,-s deployment=host -s mode=root,$(TRIDENT_VERITY_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx,artifacts/trident-verity-testimage.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-usrverity-testimage.cosi,trident-verity-testimage,-s deployment=host -s mode=usr,$(TRIDENT_VERITY_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx,artifacts/trident-usrverity-testimage.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-container-verity-testimage.cosi,trident-verity-testimage,-s deployment=container -s mode=root,$(TRIDENT_VERITY_TESTIMAGE_DEPS) artifacts/baremetal.vhdx,artifacts/trident-container-verity-testimage.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-container-usrverity-testimage.cosi,trident-verity-testimage,-s deployment=container -s mode=usr,$(TRIDENT_VERITY_TESTIMAGE_DEPS) artifacts/baremetal.vhdx,artifacts/trident-container-usrverity-testimage.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-container-testimage.cosi,trident-container-testimage,,$(TRIDENT_CONTAINER_TESTIMAGE_DEPS) artifacts/baremetal.vhdx,artifacts/trident-container-testimage.cosi,:))
$(eval $(call declare_tailor_target,artifacts/azurelinux-direct-streaming-testimage-amd64.cosi,azurelinux-direct-streaming-testimage,-s arch=amd64,$(AZURELINUX_DIRECT_STREAMING_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx,artifacts/azurelinux-direct-streaming-testimage-amd64.raw,:))
$(eval $(call declare_tailor_target,artifacts/azurelinux-direct-streaming-testimage-arm64.cosi,azurelinux-direct-streaming-testimage,-s arch=arm64,$(AZURELINUX_DIRECT_STREAMING_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_arm64.vhdx,artifacts/azurelinux-direct-streaming-testimage-arm64.raw,:))
$(eval $(call declare_tailor_target,artifacts/ubuntu-direct-streaming-testimage-2204-amd64.cosi,foreign-direct-streaming-testimage,-s arch=amd64 -s source=ubuntu-2204,$(FOREIGN_DIRECT_STREAMING_DEPS) artifacts/ubuntu_2204_amd64.vhdx,artifacts/ubuntu-direct-streaming-testimage-2204-amd64.raw,:))
$(eval $(call declare_tailor_target,artifacts/ubuntu-direct-streaming-testimage-2204-arm64.cosi,foreign-direct-streaming-testimage,-s arch=arm64 -s source=ubuntu-2204,$(FOREIGN_DIRECT_STREAMING_DEPS) artifacts/ubuntu_2204_arm64.vhdx,artifacts/ubuntu-direct-streaming-testimage-2204-arm64.raw,:))
$(eval $(call declare_tailor_target,artifacts/ubuntu-direct-streaming-testimage-2404-amd64.cosi,foreign-direct-streaming-testimage,-s arch=amd64 -s source=ubuntu-2404,$(FOREIGN_DIRECT_STREAMING_DEPS) artifacts/ubuntu_2404_amd64.vhdx,artifacts/ubuntu-direct-streaming-testimage-2404-amd64.raw,:))
$(eval $(call declare_tailor_target,artifacts/ubuntu-direct-streaming-testimage-2404-arm64.cosi,foreign-direct-streaming-testimage,-s arch=arm64 -s source=ubuntu-2404,$(FOREIGN_DIRECT_STREAMING_DEPS) artifacts/ubuntu_2404_arm64.vhdx,artifacts/ubuntu-direct-streaming-testimage-2404-arm64.raw,:))
$(eval $(call declare_tailor_target,artifacts/gb200-direct-streaming-testimage-2404-arm64.cosi,foreign-direct-streaming-testimage,-s arch=arm64 -s source=gb200-2404,$(FOREIGN_DIRECT_STREAMING_DEPS) artifacts/gb200_2404_arm64.vhdx,artifacts/gb200-direct-streaming-testimage-2404-arm64.raw,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-testimage.cosi,trident-vm-testimage,-s arch=amd64 -s scenario=grub,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-grub-testimage.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-testimage.qcow2,trident-vm-testimage,-s arch=amd64 -s scenario=grub-base,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-grub-testimage.qcow2,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-verity-testimage.cosi,trident-vm-testimage,-s arch=amd64 -s scenario=grub-verity,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-grub-verity-testimage.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-verity-testimage.qcow2,trident-vm-testimage,-s arch=amd64 -s scenario=grub-verity-base,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-grub-verity-testimage.qcow2,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-root-verity-testimage.cosi,trident-vm-testimage,-s arch=amd64 -s scenario=root-verity,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-root-verity-testimage.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-usr-verity-testimage.cosi,trident-vm-testimage,-s arch=amd64 -s scenario=usr-verity,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-usr-verity-testimage.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-usr-verity-testimage.qcow2,trident-vm-testimage,-s arch=amd64 -s scenario=usr-verity,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-usr-verity-testimage.qcow2,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-acl-agent-update-testimage.cosi,trident-vm-testimage,-s arch=amd64 -s scenario=acl-agent,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-acl-agent-update-testimage.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-acl-agent-testimage.qcow2,trident-vm-testimage,-s arch=amd64 -s scenario=acl-agent-base,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-acl-agent-testimage.qcow2,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-verity-azure-testimage.cosi,trident-vm-testimage,-s arch=amd64 -s scenario=grub-verity-azure,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_selinux.vhdx,artifacts/trident-vm-grub-verity-azure-testimage.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-verity-azure-testimage.vhd,trident-vm-testimage,-s arch=amd64 -s scenario=grub-verity-azure-base,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_selinux.vhdx,artifacts/trident-vm-grub-verity-azure-testimage.vhd,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-testimage-arm64.cosi,trident-vm-testimage,-s arch=arm64 -s scenario=grub,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_arm64.vhdx,artifacts/trident-vm-grub-testimage-arm64.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-verity-testimage-arm64.cosi,trident-vm-testimage,-s arch=arm64 -s scenario=grub-verity,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_arm64.vhdx,artifacts/trident-vm-grub-verity-testimage-arm64.cosi,:))

$(ARTIFACTS_TEST_IMAGE_DIR)/azl-installer.iso: $(TAILOR_WORKSPACE) $(AZL_INSTALLER_TAILOR_DEPS) $(TAILOR_RPM_DEPS) bin/liveinstaller artifacts/baremetal.vhdx $(ARTIFACTS_TEST_IMAGE_DIR)/regular.cosi
	rm -rf $(AZL_INSTALLER_ISO_DIR)/images
	mkdir -p $(AZL_INSTALLER_ISO_DIR)/images
	cp $(ARTIFACTS_TEST_IMAGE_DIR)/regular.cosi $(AZL_INSTALLER_ISO_DIR)/images/trident-testimage.cosi
	rm -rf $(AZL_INSTALLER_ISO_DIR)/bin
	mkdir -p $(AZL_INSTALLER_ISO_DIR)/bin
	cp bin/liveinstaller $(AZL_INSTALLER_ISO_DIR)/bin/
	rm -f $(TAILOR_DIR)/artifacts/ca_cert.pem
	$(CARGO_TAILOR) run --manifest-path tools/tailor/Cargo.toml --quiet -- -vvv --log-dir $(TAILOR_DIR)/artifacts/tailor-logs --manifest $(TAILOR_WORKSPACE) build azl-installer --output-dir $(ARTIFACTS_TEST_IMAGE_DIR)
