package scenario

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"tridenttools/storm/utils/trident"
)

// The Go copy of the health check must not drift from the one the legacy suite
// still runs, or the two suites would silently validate different things.
func TestUefiFallbackScriptMatchesLegacy(t *testing.T) {
	legacyPath := filepath.Join("..", "..", "..", "..",
		"tests", "e2e_tests", "helpers", "uefi_fallback_validation_script.txt")
	legacy, err := os.ReadFile(legacyPath)
	if err != nil {
		t.Fatalf("read legacy script: %v", err)
	}

	if string(legacy) != uefiFallbackValidationScript {
		t.Errorf("the embedded UEFI fallback script has drifted from %s", legacyPath)
	}
}

func TestResolveUefiFallbackModeIsDeterministicAndSpread(t *testing.T) {
	seen := map[string]bool{}
	for _, config := range []string{"base", "encrypted-partition", "root-verity", "raid-mirrored", "misc", "combined"} {
		s := newScenarioForTest(t, abConfig)
		s.name = config + "_vm-host"

		first := s.resolveUefiFallbackMode()
		if first != s.resolveUefiFallbackMode() {
			t.Errorf("%s: mode is not stable across calls", config)
		}
		if !slicesContains(uefiFallbackModes, first) {
			t.Errorf("%s: %q is not a valid mode", config, first)
		}
		seen[first] = true
	}

	// The point of deriving from the name is that the matrix still covers more
	// than one mode; a constant would defeat the purpose.
	if len(seen) < 2 {
		t.Errorf("expected the modes to vary across configurations, got %v", seen)
	}
}

func TestResolveUefiFallbackModeHonoursTheArgument(t *testing.T) {
	s := newScenarioForTest(t, abConfig)
	s.args.UefiFallbackMode = uefiFallbackModeOptimistic
	if got := s.resolveUefiFallbackMode(); got != uefiFallbackModeOptimistic {
		t.Errorf("got %q, want the explicitly requested mode", got)
	}
}

func TestInjectUefiFallbackValidationAddsBothChecks(t *testing.T) {
	s := newScenarioForTest(t, abConfig)
	s.name = "base_vm-host"
	s.args.UefiFallbackMode = uefiFallbackModeConservative

	if err := s.injectUefiFallbackValidation(); err != nil {
		t.Fatalf("injectUefiFallbackValidation: %v", err)
	}

	if mode, _ := s.config.S("os", "uefiFallback").Data().(string); mode != uefiFallbackModeConservative {
		t.Errorf("os.uefiFallback = %q, want conservative", mode)
	}

	byName := map[string]string{}
	runOn := map[string]string{}
	for _, check := range s.config.S("health", "checks").Children() {
		name, _ := check.S("name").Data().(string)
		content, _ := check.S("content").Data().(string)
		byName[name] = content
		for _, phase := range check.S("runOn").Children() {
			p, _ := phase.Data().(string)
			runOn[name] = p
		}
	}

	if len(byName) != 2 {
		t.Fatalf("expected 2 health checks, got %d: %v", len(byName), byName)
	}
	if runOn[uefiFallbackInstallCheckName] != "clean-install" {
		t.Errorf("install check runOn = %q", runOn[uefiFallbackInstallCheckName])
	}
	if runOn[uefiFallbackAbUpdateCheckName] != "ab-update" {
		t.Errorf("update check runOn = %q", runOn[uefiFallbackAbUpdateCheckName])
	}

	for name, content := range byName {
		if strings.Contains(content, "_REPLACE_") {
			t.Errorf("%s: unsubstituted placeholder remains", name)
		}
		if !strings.Contains(content, `"conservative"`) {
			t.Errorf("%s: mode not substituted", name)
		}
	}

	// Only the A/B update check applies the conservative opposite-entry rule;
	// on a clean install there is no previous entry to compare against.
	if !strings.Contains(byName[uefiFallbackAbUpdateCheckName], `"conservative" ] && true;`) {
		t.Error("the ab-update check should evaluate the conservative swap rule")
	}
	if !strings.Contains(byName[uefiFallbackInstallCheckName], `"conservative" ] && false;`) {
		t.Error("the clean-install check should not evaluate the conservative swap rule")
	}
}

func TestInjectUefiFallbackValidationUsesContainerRootPrefix(t *testing.T) {
	s := newScenarioForTest(t, abConfig)
	s.name = "base_vm-container"
	s.runtime = trident.RuntimeTypeContainer

	if err := s.injectUefiFallbackValidation(); err != nil {
		t.Fatalf("injectUefiFallbackValidation: %v", err)
	}

	content, _ := s.config.S("health", "checks").Children()[0].S("content").Data().(string)
	if !strings.Contains(content, `EFI_PATH="/host/boot/efi/EFI"`) {
		t.Errorf("container check should resolve paths under /host, got:\n%s", firstLines(content, 4))
	}
}

func TestInjectUefiFallbackValidationSkipsRerunAndPresetModes(t *testing.T) {
	// `rerun` plus these checks exceeds what fits in the ISO.
	rerun := newScenarioForTest(t, abConfig)
	rerun.name = "rerun_vm-host"
	if err := rerun.injectUefiFallbackValidation(); err != nil {
		t.Fatalf("injectUefiFallbackValidation: %v", err)
	}
	if rerun.config.Exists("os", "uefiFallback") || rerun.config.Exists("health", "checks") {
		t.Error("nothing should be injected for the rerun configuration")
	}

	// A configuration that pins a mode deliberately must not be overridden.
	preset := newScenarioForTest(t, abConfig)
	preset.name = "base_vm-host"
	preset.config.Set(uefiFallbackModeDisabled, "os", "uefiFallback")
	if err := preset.injectUefiFallbackValidation(); err != nil {
		t.Fatalf("injectUefiFallbackValidation: %v", err)
	}
	if mode, _ := preset.config.S("os", "uefiFallback").Data().(string); mode != uefiFallbackModeDisabled {
		t.Errorf("preset mode was overridden, got %q", mode)
	}
	if preset.config.Exists("health", "checks") {
		t.Error("no checks should be injected when the mode is already pinned")
	}
}

// The auto-rollback suite strips its own failing checks before the second A/B
// update. It must not take the UEFI checks with them, or every update after the
// rollback would stop validating the fallback.
func TestUefiFallbackChecksSurviveRollbackCleanup(t *testing.T) {
	s := newScenarioForTest(t, abConfig)
	s.name = "base_vm-host"
	if err := s.injectUefiFallbackValidation(); err != nil {
		t.Fatalf("injectUefiFallbackValidation: %v", err)
	}
	if err := s.injectRollbackHealthChecks(nil); err != nil {
		t.Fatalf("injectRollbackHealthChecks: %v", err)
	}
	if err := s.removeRollbackHealthChecks(nil); err != nil {
		t.Fatalf("removeRollbackHealthChecks: %v", err)
	}

	var names []string
	for _, check := range s.config.S("health", "checks").Children() {
		name, _ := check.S("name").Data().(string)
		names = append(names, name)
	}

	for _, want := range []string{uefiFallbackInstallCheckName, uefiFallbackAbUpdateCheckName} {
		if !slicesContains(names, want) {
			t.Errorf("%s was removed by the rollback cleanup; remaining: %v", want, names)
		}
	}
	if len(names) != 2 {
		t.Errorf("expected only the two UEFI checks to remain, got %v", names)
	}
}

func slicesContains(haystack []string, needle string) bool {
	for _, v := range haystack {
		if v == needle {
			return true
		}
	}
	return false
}

func firstLines(s string, n int) string {
	lines := strings.SplitN(s, "\n", n+1)
	if len(lines) > n {
		lines = lines[:n]
	}
	return strings.Join(lines, "\n")
}
