package tests

import (
	"reflect"
	"strings"
	"testing"

	stormrollbackconfig "tridenttools/storm/rollback/utils/config"
	stormvmconfig "tridenttools/storm/utils/vm/config"
)

func TestImageCustomizerContainerArgs(t *testing.T) {
	config := stormrollbackconfig.TestConfig{ArtifactsDir: "/owned artifacts", ImageCustomizerImage: "image@sha256:pinned"}
	legacy := []string{
		"run", "--pull=never", "--rm", "--privileged",
		"-v", "/owned image:/input-image:z",
		"-v", "/owned artifacts:/artifacts",
		"-v", "/dev:/dev", "image@sha256:pinned",
		"--log-level", "debug", "--build-dir", "/build",
		"--image-file", "/input-image/base.qcow2",
		"--output-image-file", "/artifacts/adjusted.qcow2",
		"--output-image-format", "qcow2",
		"--config-file", "/artifacts/config.yaml",
	}
	for _, name := range []string{"", "trident-lifecycle-123-mic"} {
		t.Run("name="+name, func(t *testing.T) {
			hook, err := imageCustomizerContainerArgs(name)
			if err != nil {
				t.Fatal(err)
			}
			actual := imageCustomizerRunArgs(config, "/owned image", "base.qcow2", "adjusted.qcow2", "config.yaml", hook)
			expected := append([]string{}, legacy[:4]...)
			if name != "" {
				expected = append(expected, "--name", name, "--label", "com.microsoft.trident.lifecycle-owner="+name)
			}
			expected = append(expected, legacy[4:]...)
			if !reflect.DeepEqual(actual, expected) {
				t.Fatalf("Docker argv = %#v, want %#v", actual, expected)
			}
		})
	}
}

func TestImageCustomizerRejectsInvalidNameBeforePreparation(t *testing.T) {
	for _, name := range []string{"--name", "/name", "a b", "a\nb", "a=b", strings.Repeat("a", 129)} {
		t.Run(name, func(t *testing.T) {
			err := PrepareQcow2(stormrollbackconfig.TestConfig{
				ImageCustomizerContainerName: name,
				SkipExtensionTesting:         true,
			}, stormvmconfig.AllVMConfig{})
			if err == nil || !strings.Contains(err.Error(), "invalid Image Customizer container name") {
				t.Fatalf("unexpected validation result: %v", err)
			}
		})
	}
}

func TestImageCustomizerDefaultSkippedPreparationUnchanged(t *testing.T) {
	if err := PrepareQcow2(stormrollbackconfig.TestConfig{SkipExtensionTesting: true}, stormvmconfig.AllVMConfig{}); err != nil {
		t.Fatal(err)
	}
}
