CARGO_TAILOR ?= $(if $(shell command -v cargo 2>/dev/null),cargo,$(HOME)/.cargo/bin/cargo)
TAILOR_DIR := $(patsubst %/,%,$(dir $(lastword $(MAKEFILE_LIST))))
TAILOR_WORKSPACE := $(TAILOR_DIR)/tailor.yaml

# Matches tailor.yaml's default `ic` toolchain entry. Override to build against
# a dev imagecustomizer; only overridden values trigger the toolchain swap below.
TAILOR_DEFAULT_IC_IMAGE := mcr.microsoft.com/azurelinux/imagecustomizer:latest
MIC_CONTAINER_IMAGE ?= $(TAILOR_DEFAULT_IC_IMAGE)
TAILOR_RPM_DEPS := bin/RPMS $(shell find bin/RPMS -name '*.rpm' 2>/dev/null) artifacts/rpm-overrides $(shell find artifacts/rpm-overrides -name '*.rpm' 2>/dev/null)
VM_IMAGE_PATH_PREFIX := $(TAILOR_DIR)/trident-vm-testimage/base

# Shared files under common/ are referenced (via extraDependencies) by every
# family below; depend on them everywhere so edits there trigger a rebuild.
TAILOR_COMMON_DEPS := $(shell find $(TAILOR_DIR)/common 2>/dev/null)

TRIDENT_INSTALLER_DEPS := $(shell find $(TAILOR_DIR)/trident-installer 2>/dev/null) $(TAILOR_COMMON_DEPS)
TRIDENT_TESTIMAGE_DEPS := $(shell find $(TAILOR_DIR)/trident-testimage 2>/dev/null) $(TAILOR_COMMON_DEPS)
TRIDENT_VERITY_TESTIMAGE_DEPS := $(shell find $(TAILOR_DIR)/trident-verity-testimage 2>/dev/null) $(TAILOR_COMMON_DEPS)
TRIDENT_CONTAINER_INSTALLER_DEPS := $(shell find $(TAILOR_DIR)/trident-container-installer 2>/dev/null) $(TAILOR_COMMON_DEPS)
TRIDENT_CONTAINER_TESTIMAGE_DEPS := $(shell find $(TAILOR_DIR)/trident-container-testimage 2>/dev/null) $(TAILOR_COMMON_DEPS)
TRIDENT_FUNCTEST_DEPS := $(shell find $(TAILOR_DIR)/trident-functest 2>/dev/null)
AZL_INSTALLER_TAILOR_DEPS := $(shell find $(AZL_INSTALLER_DIR) 2>/dev/null) $(TAILOR_COMMON_DEPS)
AZURELINUX_DIRECT_STREAMING_DEPS := $(shell find $(TAILOR_DIR)/azurelinux-direct-streaming-testimage 2>/dev/null) $(TAILOR_COMMON_DEPS)
FOREIGN_DIRECT_STREAMING_DEPS := $(shell find $(TAILOR_DIR)/foreign-direct-streaming-testimage 2>/dev/null)
TRIDENT_VM_TESTIMAGE_DEPS := $(shell find $(TAILOR_DIR)/trident-vm-testimage 2>/dev/null) $(VM_IMAGE_PATH_PREFIX)/files/id_rsa.pub $(TAILOR_COMMON_DEPS)
TRIDENT_MOS_DEPS := $(shell find $(TAILOR_DIR)/trident-mos 2>/dev/null) $(TAILOR_COMMON_DEPS)

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

# Every declare_tailor_target recipe invokes tailor with the same
# `--output-dir artifacts/`, and tailor takes a nonblocking exclusive lock on
# that output directory (`another tailor build is already running` otherwise)
# -- so under `make -j all-cosi`/`all-iso`, every invocation but the first to
# acquire tailor's own lock would fail outright instead of queuing. Wrap just
# the tailor invocation in a blocking flock on a shared lock file so parallel
# Make jobs serialize through tailor one at a time instead of racing it.
define declare_tailor_target
$1: $(TAILOR_WORKSPACE) $4
	@mkdir -p artifacts
	@echo "Building '$$(@F)' [$@]"
	@echo "Prerequisites:"
	@echo "$$^" | tr ' ' '\n' | sed 's/^/    /'
	rm -f $(call tailor_ca_cert_staging_path,$1)
	@manifest="$(TAILOR_DIR)/.tailor.local.$(subst /,_,$1).yaml"; \
	sed -e 's@^      publishCaCert: \./artifacts/ca_cert\.pem$$$$@      publishCaCert: ./artifacts/ca_cert.$(subst /,_,$1).pem@' \
	    "$(TAILOR_WORKSPACE)" > "$$$$manifest"; \
	if [ "$(MIC_CONTAINER_IMAGE)" != "$(TAILOR_DEFAULT_IC_IMAGE)" ]; then \
		mic="$(MIC_CONTAINER_IMAGE)"; image="$$$${mic%:*}"; tag="$$$${mic##*:}"; \
		if [ "$$$$image" = "$$$$tag" ]; then image="$$$$mic"; tag="latest"; fi; \
		sed -e "s|^  default: ic$$$$|  default: ic-dev|" \
		    -e "s|^      tag: latest$$$$|      tag: latest\n    - name: ic-dev\n      container: $$$$image\n      tag: $$$$tag\n      pull: never|" \
		    "$$$$manifest" > "$$$$manifest.tmp" && mv "$$$$manifest.tmp" "$$$$manifest"; \
	fi; \
	flock artifacts/.tailor-build.flock $(CARGO_TAILOR) run --manifest-path tools/tailor/Cargo.toml --quiet -- -vvv --log-dir $(TAILOR_DIR)/artifacts/tailor-logs --manifest "$$$$manifest" build $2 $3 --output-dir $(dir $1)
	@rm -f $(TAILOR_DIR)/.tailor.local.$(subst /,_,$1).yaml
	@if [ -f $(call tailor_ca_cert_staging_path,$1) ]; then cp $(call tailor_ca_cert_staging_path,$1) "$(dir $1)/ca_cert.pem"; fi
	@if [ "$1" != "$5" ]; then cp "$5" "$1"; fi
	@$6
endef

# NOTE on signed targets and `make -j`: each signed target's own certificate
# always lands, race-free, at `artifacts/ca_cert.<target>.pem`
# (tailor_ca_cert_staging_path, above). The extra copy to the shared
# `artifacts/ca_cert.pem` compat path (for single-target builds and the CI
# templates, which only ever build one signed target per job) is NOT safe to
# rely on when `make -j` builds more than one signed target together
# (currently trident-usrverity-testimage and trident-container-usrverity-testimage,
# both carrying the `test-ca` signing profile): a fresh CA is minted per
# invocation, so the shared path ends up with whichever target finished last.
# Build signed targets one at a time (or read each target's own
# `artifacts/ca_cert.<target>.pem`) when that matters.

# NETLAUNCH_ISO (root Makefile) defaults to bin/trident-mos.iso -- output
# path kept outside artifacts/ for legacy/netlaunch-tooling compatibility,
# same as every other declare_tailor_target: $(dir $1) becomes the
# --output-dir, so the tailor-produced bin/trident-mos_amd64_iso.iso is
# renamed in place to the legacy bin/trident-mos.iso.
$(eval $(call declare_tailor_target,bin/trident-mos.iso,trident-mos,,$(TRIDENT_MOS_DEPS) artifacts/baremetal.vhdx artifacts/rcp-agent,bin/trident-mos_amd64_iso.iso,:))

$(eval $(call declare_tailor_target,artifacts/trident-installer.iso,trident-installer,-s arch=amd64 -s variant=default,$(TRIDENT_INSTALLER_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx,artifacts/trident-installer_amd64_default_iso.iso,:))
$(eval $(call declare_tailor_target,artifacts/trident-split-installer.iso,trident-installer,-s arch=amd64 -s variant=split,$(TRIDENT_INSTALLER_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx,artifacts/trident-installer_amd64_split_iso.iso,:))
$(eval $(call declare_tailor_target,artifacts/trident-installer-arm64.iso,trident-installer,-s arch=arm64 -s variant=default,$(TRIDENT_INSTALLER_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_arm64.vhdx,artifacts/trident-installer_arm64_default_iso.iso,:))
$(eval $(call declare_tailor_target,artifacts/trident-direct-streaming-installer-amd64.iso,trident-installer,-s arch=amd64 -s variant=direct-streaming,$(TRIDENT_INSTALLER_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx artifacts/rcp-agent artifacts/rcp-agent.service,artifacts/trident-installer_amd64_direct-streaming_iso.iso,:))
$(eval $(call declare_tailor_target,artifacts/trident-direct-streaming-installer-arm64.iso,trident-installer,-s arch=arm64 -s variant=direct-streaming,$(TRIDENT_INSTALLER_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_arm64.vhdx artifacts/rcp-agent artifacts/rcp-agent.service,artifacts/trident-installer_arm64_direct-streaming_iso.iso,:))
$(eval $(call declare_tailor_target,artifacts/trident-container-installer.iso,trident-container-installer,,$(TRIDENT_CONTAINER_INSTALLER_DEPS) artifacts/baremetal.vhdx,artifacts/trident-container-installer_amd64_iso.iso,:))
$(eval $(call declare_tailor_target,artifacts/trident-functest.qcow2,trident-functest,,$(TRIDENT_FUNCTEST_DEPS) artifacts/baremetal.vhdx,artifacts/trident-functest_amd64_qcow2.qcow2,:))
$(eval $(call declare_tailor_target,artifacts/trident-testimage.cosi,trident-testimage,-s arch=amd64,$(TRIDENT_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx,artifacts/trident-testimage_amd64_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-testimage-arm64.cosi,trident-testimage,-s arch=arm64,$(TRIDENT_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_arm64.vhdx,artifacts/trident-testimage_arm64_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-verity-testimage.cosi,trident-verity-testimage,-s deployment=host -s mode=root,$(TRIDENT_VERITY_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx,artifacts/trident-verity-testimage_host_root_amd64_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-usrverity-testimage.cosi,trident-verity-testimage,-s deployment=host -s mode=usr,$(TRIDENT_VERITY_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx,artifacts/trident-verity-testimage_host_usr_amd64_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-container-verity-testimage.cosi,trident-verity-testimage,-s deployment=container -s mode=root,$(TRIDENT_VERITY_TESTIMAGE_DEPS) artifacts/baremetal.vhdx,artifacts/trident-verity-testimage_container_root_amd64_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-container-usrverity-testimage.cosi,trident-verity-testimage,-s deployment=container -s mode=usr,$(TRIDENT_VERITY_TESTIMAGE_DEPS) artifacts/baremetal.vhdx,artifacts/trident-verity-testimage_container_usr_amd64_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-container-testimage.cosi,trident-container-testimage,,$(TRIDENT_CONTAINER_TESTIMAGE_DEPS) artifacts/baremetal.vhdx,artifacts/trident-container-testimage_amd64_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/azurelinux-direct-streaming-testimage-amd64.cosi,azurelinux-direct-streaming-testimage,-s arch=amd64,$(AZURELINUX_DIRECT_STREAMING_DEPS) $(TAILOR_RPM_DEPS) artifacts/baremetal.vhdx,artifacts/azurelinux-direct-streaming-testimage_amd64_baremetal-image.raw,:))
$(eval $(call declare_tailor_target,artifacts/azurelinux-direct-streaming-testimage-arm64.cosi,azurelinux-direct-streaming-testimage,-s arch=arm64,$(AZURELINUX_DIRECT_STREAMING_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_arm64.vhdx,artifacts/azurelinux-direct-streaming-testimage_arm64_baremetal-image.raw,:))
$(eval $(call declare_tailor_target,artifacts/ubuntu-direct-streaming-testimage-2204-amd64.cosi,foreign-direct-streaming-testimage,-s arch=amd64 -s source=ubuntu-2204,$(FOREIGN_DIRECT_STREAMING_DEPS) artifacts/ubuntu_2204_amd64.vhdx,artifacts/foreign-direct-streaming-testimage_amd64_ubuntu-2204_baremetal-image.raw,:))
$(eval $(call declare_tailor_target,artifacts/ubuntu-direct-streaming-testimage-2204-arm64.cosi,foreign-direct-streaming-testimage,-s arch=arm64 -s source=ubuntu-2204,$(FOREIGN_DIRECT_STREAMING_DEPS) artifacts/ubuntu_2204_arm64.vhdx,artifacts/foreign-direct-streaming-testimage_arm64_ubuntu-2204_baremetal-image.raw,:))
$(eval $(call declare_tailor_target,artifacts/ubuntu-direct-streaming-testimage-2404-amd64.cosi,foreign-direct-streaming-testimage,-s arch=amd64 -s source=ubuntu-2404,$(FOREIGN_DIRECT_STREAMING_DEPS) artifacts/ubuntu_2404_amd64.vhdx,artifacts/foreign-direct-streaming-testimage_amd64_ubuntu-2404_baremetal-image.raw,:))
$(eval $(call declare_tailor_target,artifacts/ubuntu-direct-streaming-testimage-2404-arm64.cosi,foreign-direct-streaming-testimage,-s arch=arm64 -s source=ubuntu-2404,$(FOREIGN_DIRECT_STREAMING_DEPS) artifacts/ubuntu_2404_arm64.vhdx,artifacts/foreign-direct-streaming-testimage_arm64_ubuntu-2404_baremetal-image.raw,:))
$(eval $(call declare_tailor_target,artifacts/gb200-direct-streaming-testimage-2404-arm64.cosi,foreign-direct-streaming-testimage,-s arch=arm64 -s source=gb200-2404,$(FOREIGN_DIRECT_STREAMING_DEPS) artifacts/gb200_2404_arm64.vhdx,artifacts/foreign-direct-streaming-testimage_arm64_gb200-2404_baremetal-image.raw,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-testimage.cosi,trident-vm-testimage,-s arch=amd64 -s scenario=grub,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-testimage_amd64_grub_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-testimage.qcow2,trident-vm-testimage,-s arch=amd64 -s scenario=grub-base,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-testimage_amd64_grub-base_qcow2.qcow2,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-verity-testimage.cosi,trident-vm-testimage,-s arch=amd64 -s scenario=grub-verity,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-testimage_amd64_grub-verity_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-verity-testimage.qcow2,trident-vm-testimage,-s arch=amd64 -s scenario=grub-verity-base,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-testimage_amd64_grub-verity-base_qcow2.qcow2,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-root-verity-testimage.cosi,trident-vm-testimage,-s arch=amd64 -s scenario=root-verity,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-testimage_amd64_root-verity_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-root-verity-testimage.qcow2,trident-vm-testimage,-s arch=amd64 -s scenario=root-verity,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-testimage_amd64_root-verity_qcow2.qcow2,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-usr-verity-testimage.cosi,trident-vm-testimage,-s arch=amd64 -s scenario=usr-verity,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-testimage_amd64_usr-verity_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-usr-verity-testimage.qcow2,trident-vm-testimage,-s arch=amd64 -s scenario=usr-verity,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-testimage_amd64_usr-verity_qcow2.qcow2,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-acl-agent-update-testimage.cosi,trident-vm-testimage,-s arch=amd64 -s scenario=acl-agent,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-testimage_amd64_acl-agent_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-acl-agent-testimage.qcow2,trident-vm-testimage,-s arch=amd64 -s scenario=acl-agent-base,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/qemu_guest.vhdx,artifacts/trident-vm-testimage_amd64_acl-agent-base_qcow2.qcow2,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-verity-azure-testimage.cosi,trident-vm-testimage,-s arch=amd64 -s scenario=grub-verity-azure,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_selinux.vhdx,artifacts/trident-vm-testimage_amd64_grub-verity-azure_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-verity-azure-testimage.vhd,trident-vm-testimage,-s arch=amd64 -s scenario=grub-verity-azure-base,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_selinux.vhdx,artifacts/trident-vm-testimage_amd64_grub-verity-azure-base_vhd-fixed.vhd,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-testimage-arm64.cosi,trident-vm-testimage,-s arch=arm64 -s scenario=grub,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_arm64.vhdx,artifacts/trident-vm-testimage_arm64_grub_cosi.cosi,:))
$(eval $(call declare_tailor_target,artifacts/trident-vm-grub-verity-testimage-arm64.cosi,trident-vm-testimage,-s arch=arm64 -s scenario=grub-verity,$(TRIDENT_VM_TESTIMAGE_DEPS) $(TAILOR_RPM_DEPS) artifacts/core_arm64.vhdx,artifacts/trident-vm-testimage_arm64_grub-verity_cosi.cosi,:))

$(ARTIFACTS_TEST_IMAGE_DIR)/azl-installer.iso: $(TAILOR_WORKSPACE) $(AZL_INSTALLER_TAILOR_DEPS) $(TAILOR_RPM_DEPS) bin/liveinstaller artifacts/baremetal.vhdx $(ARTIFACTS_TEST_IMAGE_DIR)/regular.cosi
	rm -rf $(AZL_INSTALLER_ISO_DIR)/images
	mkdir -p $(AZL_INSTALLER_ISO_DIR)/images
	cp $(ARTIFACTS_TEST_IMAGE_DIR)/regular.cosi $(AZL_INSTALLER_ISO_DIR)/images/trident-testimage.cosi
	rm -rf $(AZL_INSTALLER_ISO_DIR)/bin
	mkdir -p $(AZL_INSTALLER_ISO_DIR)/bin
	cp bin/liveinstaller $(AZL_INSTALLER_ISO_DIR)/bin/
	rm -f $(TAILOR_DIR)/artifacts/ca_cert.pem
	@manifest="$(TAILOR_WORKSPACE)"; \
	if [ "$(MIC_CONTAINER_IMAGE)" != "$(TAILOR_DEFAULT_IC_IMAGE)" ]; then \
		manifest="$(TAILOR_DIR)/.tailor.local.azl-installer.yaml"; \
		mic="$(MIC_CONTAINER_IMAGE)"; image="$${mic%:*}"; tag="$${mic##*:}"; \
		if [ "$$image" = "$$tag" ]; then image="$$mic"; tag="latest"; fi; \
		sed -e "s|^  default: ic$$|  default: ic-dev|" \
		    -e "s|^      tag: latest$$|      tag: latest\n    - name: ic-dev\n      container: $$image\n      tag: $$tag\n      pull: never|" \
		    "$(TAILOR_WORKSPACE)" > "$$manifest"; \
	fi; \
	$(CARGO_TAILOR) run --manifest-path tools/tailor/Cargo.toml --quiet -- -vvv --log-dir $(TAILOR_DIR)/artifacts/tailor-logs --manifest "$$manifest" build azl-installer --output-dir $(ARTIFACTS_TEST_IMAGE_DIR)
	@rm -f $(TAILOR_DIR)/.tailor.local.azl-installer.yaml
	mv $(ARTIFACTS_TEST_IMAGE_DIR)/azl-installer_amd64_iso.iso $@
