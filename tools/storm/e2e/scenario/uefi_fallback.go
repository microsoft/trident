package scenario

import (
	"fmt"
	"hash/fnv"
	"strings"

	"tridenttools/storm/utils/trident"

	"github.com/sirupsen/logrus"
)

// UEFI fallback validation, ported from the legacy pipeline's
// tests/e2e_tests/helpers/inject_uefi_fallback_validation.py (wired at
// .pipelines/templates/stages/testing_common/trident-prep.yml:63-69).
//
// base_test.py::test_uefi_fallback (ported to validate.ValidateUefiFallback) was
// only ever the weaker half of this coverage. The substantive check is this
// health check, which Trident runs ON the target OS after both clean install and
// A/B update, and which encodes the rule the SSH-side check cannot see: under
// `conservative`, an A/B update must leave the fallback directory matching the
// *opposite* AZL boot entry, because the fallback is only advanced once the new
// entry is known good.
const (
	uefiFallbackModeDisabled     = "disabled"
	uefiFallbackModeConservative = "conservative"
	uefiFallbackModeOptimistic   = "optimistic"

	uefiFallbackInstallCheckName  = "uefi-fallback-validation-install"
	uefiFallbackAbUpdateCheckName = "uefi-fallback-validation-update"

	// The `rerun` config is excluded: its Host Configuration plus these checks
	// exceeds what can be injected into the ISO. Legacy skipped it for the same
	// reason.
	uefiFallbackExcludedConfig = "rerun"

	// Under a containerized Trident the health check runs inside the container,
	// where the host filesystem is mounted at /host.
	uefiFallbackContainerRootPrefix = "/host"
)

// uefiFallbackModes is the set a mode is chosen from when none is configured.
// Ordered, because the automatic choice indexes into it and must be stable.
var uefiFallbackModes = []string{
	uefiFallbackModeDisabled,
	uefiFallbackModeConservative,
	uefiFallbackModeOptimistic,
}

// uefiFallbackValidationScript is a verbatim copy of
// tests/e2e_tests/helpers/uefi_fallback_validation_script.txt. The placeholder
// tokens are kept so the two remain diffable while the legacy suite still runs;
// they are substituted below exactly as the Python does.
const uefiFallbackValidationScript = `#!/bin/sh

EFI_PATH="_REPLACE_ROOT_PREFIX_/boot/efi/EFI"
FALLBACK_PATH="$EFI_PATH/BOOT"
CURRENT_BOOT_ENTRY="$(efibootmgr | grep "BootCurrent" | head -n 1 | tr '\t' ' '| cut -d' ' -f2)"
if [ -z "$CURRENT_BOOT_ENTRY" ]; then
    echo "Failed to parse current boot entry from [$CURRENT_BOOT]"
    exit 1
fi

CURRENT_AZL_BOOT_NAME="$(efibootmgr | grep "Boot${CURRENT_BOOT_ENTRY}" | head -n 1 | tr '\t' ' '| cut -d' ' -f2 | grep "AZL")"
if [ -z "$CURRENT_AZL_BOOT_NAME" ]; then
    echo "Current boot entry is not an AZL boot entry (from [$CURRENT_BOOT_ENTRY])"
    exit 1
fi

if [ "_REPLACE_FALLBACK_NODE_" == "disabled" ]; then
    # if disabled, check that $FALLBACK_PATH is empty
    if sudo find $FALLBACK_PATH/*; then
        echo "$FALLBACK_PATH is not empty"
        exit 1
    else
        echo "$FALLBACK_PATH is empty"
        exit 0
    fi
else
    AZL_BOOT_NAME_TO_CHECK="$CURRENT_AZL_BOOT_NAME"
    if [ "_REPLACE_FALLBACK_NODE_" == "conservative" ] && _REPLACE_NOT_INSTALL_; then
        # if conservative, check that $FALLBACK_PATH == opposite of $EFI_PATH/$CURRENT_AZL_BOOT_NAME
        AZL_BOOT_NAME_TO_CHECK="$(echo "$CURRENT_AZL_BOOT_NAME" | sed "s/AZLA/AZLA_TMP/g; s/AZLB/AZLA/g; s/AZLA_TMP/AZLB/g")"
    fi

    if diff "$FALLBACK_PATH/" "$EFI_PATH/$AZL_BOOT_NAME_TO_CHECK/"; then
        echo "no difference detected between $FALLBACK_PATH and $EFI_PATH/$AZL_BOOT_NAME_TO_CHECK/"
        exit 0
    else
        echo "difference detected between $FALLBACK_PATH and $EFI_PATH/$AZL_BOOT_NAME_TO_CHECK/"
        exit 1
    fi
fi
`

// configName recovers the Host Configuration name from the scenario name, which
// discover.go composes as "<config>_<hardware>-<runtime>".
func (s *TridentE2EScenario) configName() string {
	return strings.TrimSuffix(s.name, fmt.Sprintf("_%s-%s", s.hardware, s.runtime))
}

// resolveUefiFallbackMode picks the mode to exercise. An explicit scenario
// argument wins; otherwise the mode is derived from the configuration name.
//
// Legacy picked at random per run, which spread coverage across the matrix but
// made a failure impossible to reproduce without knowing which mode had been
// drawn. Deriving from the configuration name keeps the spread (different
// configs get different modes) while making any given scenario reproducible.
func (s *TridentE2EScenario) resolveUefiFallbackMode() string {
	if s.args.UefiFallbackMode != "" {
		return s.args.UefiFallbackMode
	}

	h := fnv.New32a()
	// Hash writes never fail.
	_, _ = h.Write([]byte(s.configName()))
	return uefiFallbackModes[h.Sum32()%uint32(len(uefiFallbackModes))]
}

// renderUefiFallbackCheck substitutes the script's placeholders for one phase.
// notInstall is the literal the script evaluates as a command: "true" for the
// A/B update check (where the conservative opposite-entry rule applies) and
// "false" for the clean install.
func renderUefiFallbackCheck(mode string, rootPrefix string, notInstall string) string {
	return strings.NewReplacer(
		"_REPLACE_FALLBACK_NODE_", mode,
		"_REPLACE_ROOT_PREFIX_", rootPrefix,
		"_REPLACE_NOT_INSTALL_", notInstall,
	).Replace(uefiFallbackValidationScript)
}

// injectUefiFallbackValidation sets os.uefiFallback and appends the two health
// checks that validate it, so Trident itself asserts the fallback state on the
// target OS after each servicing operation.
//
// It is a no-op when the configuration already pins a mode, mirroring the
// Python, so a configuration that deliberately exercises a specific mode is
// never overridden.
func (s *TridentE2EScenario) injectUefiFallbackValidation() error {
	if s.configName() == uefiFallbackExcludedConfig {
		logrus.Infof("Skipping UEFI fallback validation for the %q configuration: "+
			"the resulting Host Configuration is too large to inject into the ISO",
			uefiFallbackExcludedConfig)
		return nil
	}

	if s.config.Exists("os", "uefiFallback") {
		logrus.Info("Host Configuration already sets os.uefiFallback; not injecting UEFI fallback validation")
		return nil
	}

	mode := s.resolveUefiFallbackMode()
	s.config.Set(mode, "os", "uefiFallback")

	rootPrefix := ""
	if s.runtime == trident.RuntimeTypeContainer {
		rootPrefix = uefiFallbackContainerRootPrefix
	}

	checks := []map[string]interface{}{
		{
			"name":    uefiFallbackAbUpdateCheckName,
			"content": renderUefiFallbackCheck(mode, rootPrefix, "true"),
			"runOn":   []interface{}{"ab-update"},
		},
		{
			"name":    uefiFallbackInstallCheckName,
			"content": renderUefiFallbackCheck(mode, rootPrefix, "false"),
			"runOn":   []interface{}{"clean-install"},
		},
	}
	for _, check := range checks {
		if err := s.config.ArrayAppend(check, "health", "checks"); err != nil {
			return fmt.Errorf("failed to add health check %q: %w", check["name"], err)
		}
	}

	logrus.Infof("Injected UEFI fallback validation in %q mode", mode)
	return nil
}
