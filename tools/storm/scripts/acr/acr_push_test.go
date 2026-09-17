package acr

import (
	"path/filepath"
	"strings"
	"testing"
)

// planFor is the single place that decides what a configuration hosts in ACR.
// Both acr-push and acr-delete read it, so a mistake here either leaks images
// or, worse, installs the wrong one.
func TestPlanForResolvesPerConfiguration(t *testing.T) {
	const src = "/src"

	tests := []struct {
		config   string
		want     bool
		repo     string
		numFiles int
		emitUrl  bool
	}{
		{config: "extensions", want: true, repo: "sysext-storm-host", numFiles: 2, emitUrl: false},
		{config: "misc", want: true, repo: "cosi-storm-host", numFiles: 1, emitUrl: true},
		{config: "base", want: false},
		{config: "raid-small", want: false},
	}

	for _, tt := range tests {
		t.Run(tt.config, func(t *testing.T) {
			s := AcrPushScript{Config: tt.config, RuntimeEnv: "host", SourceDir: src}
			plan, ok := s.planFor()
			if ok != tt.want {
				t.Fatalf("planFor(%q) needed=%v, want %v", tt.config, ok, tt.want)
			}
			if !tt.want {
				return
			}
			if plan.repoName != tt.repo {
				t.Errorf("repo = %q, want %q", plan.repoName, tt.repo)
			}
			if len(plan.files) != tt.numFiles {
				t.Errorf("files = %d, want %d", len(plan.files), tt.numFiles)
			}
			if plan.emitUrl != tt.emitUrl {
				t.Errorf("emitUrl = %v, want %v", plan.emitUrl, tt.emitUrl)
			}
			for _, f := range plan.files {
				if !filepath.IsAbs(f) || !strings.HasPrefix(f, src) {
					t.Errorf("file %q is not rooted at the source dir", f)
				}
			}
		})
	}
}

// Emitting an OCI URL for extensions would point image.url at a sysext image
// and replace the OS image entirely, so only an installing configuration may
// set it.
func TestOnlyInstallingConfigsEmitAnImageUrl(t *testing.T) {
	for _, cfg := range []string{"extensions", "misc"} {
		s := AcrPushScript{Config: cfg, RuntimeEnv: "container", SourceDir: "/src"}
		plan, ok := s.planFor()
		if !ok {
			t.Fatalf("%s should need a push", cfg)
		}
		if cfg == "extensions" && plan.emitUrl {
			t.Error("extensions must not emit an image URL")
		}
		if cfg == "misc" && !plan.emitUrl {
			t.Error("misc installs from the pushed image and must emit its URL")
		}
	}

	// The URL must be the ".1" tag, which is what pushFiles assigns the first
	// file and what the scenario installs from.
	if got := ociUrl("acr", "cosi-storm-host", "v123.misc.virtualMachine"); got != "oci://acr.azurecr.io/cosi-storm-host:v123.misc.virtualMachine.1" {
		t.Errorf("ociUrl = %q", got)
	}
}
