package tests

import (
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	stormrollbackconfig "tridenttools/storm/rollback/utils/config"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"gopkg.in/yaml.v2"
)

func TestPrepareQcow2PreviewFeatures(t *testing.T) {
	for _, tc := range []struct {
		name  string
		image string
		uki   bool
	}{
		{name: "azl3", image: "trident-vm-grub-testimage.qcow2"},
		{name: "azl4", image: "trident-vm-grub-azl4-testimage.qcow2"},
		{name: "uki", image: "trident-vm-root-verity-testimage.qcow2", uki: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			artifacts := t.TempDir()
			for _, name := range []string{tc.image, "test-sysext-1.raw"} {
				if err := os.WriteFile(filepath.Join(artifacts, name), nil, 0644); err != nil {
					t.Fatal(err)
				}
			}

			// Stop after config generation without invoking Docker or modifying an image.
			commands := t.TempDir()
			if err := os.WriteFile(filepath.Join(commands, "docker"), []byte("#!/bin/sh\nexit 1\n"), 0755); err != nil {
				t.Fatal(err)
			}
			t.Setenv("PATH", commands)

			err := PrepareQcow2(stormrollbackconfig.TestConfig{
				ArtifactsDir:  artifacts,
				ExtensionName: "test-sysext",
				Uki:           tc.uki,
			}, stormvmconfig.AllVMConfig{})
			if err == nil || !strings.Contains(err.Error(), "failed to pull Image Customizer image") {
				t.Fatalf("expected the Docker stub to stop preparation, got %v", err)
			}

			content, err := os.ReadFile(filepath.Join(artifacts, "image-customizer-config.yaml"))
			if err != nil {
				t.Fatal(err)
			}
			var config struct {
				PreviewFeatures []string `yaml:"previewFeatures"`
			}
			if err := yaml.Unmarshal(content, &config); err != nil {
				t.Fatal(err)
			}
			want := []string{"reinitialize-verity", "preview-distro-version"}
			if tc.uki {
				want = append(want, "uki")
			}
			if !reflect.DeepEqual(config.PreviewFeatures, want) {
				t.Fatalf("preview features = %v, want %v", config.PreviewFeatures, want)
			}
		})
	}
}

func TestPrepareQcow2SkipsExtensionPreparation(t *testing.T) {
	err := PrepareQcow2(stormrollbackconfig.TestConfig{
		SkipExtensionTesting: true,
	}, stormvmconfig.AllVMConfig{})
	if err != nil {
		t.Fatal(err)
	}
}
