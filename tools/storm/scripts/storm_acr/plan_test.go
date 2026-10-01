package storm_acr

import (
	"path/filepath"
	"strings"
	"testing"
)

// planFor is the single place deciding what a configuration hosts in ACR; both
// the push and the cleanup read it, so a mistake here either leaks images or
// acts on the wrong repository.
func TestPlanForResolvesPerConfiguration(t *testing.T) {
	const src = "/src"
	tests := []struct {
		config   string
		want     bool
		repo     string
		numFiles int
		emitUrl  bool
		emitRepo bool
	}{
		{config: "extensions", want: true, repo: "sysext-storm-host", numFiles: 2, emitRepo: true},
		{config: "misc", want: true, repo: "cosi-storm-host", numFiles: 4, emitUrl: true},
		{config: "base", want: false},
		{config: "raid-small", want: false},
	}
	for _, tt := range tests {
		t.Run(tt.config, func(t *testing.T) {
			plan, ok := planFor(tt.config, "host", src)
			if ok != tt.want {
				t.Fatalf("needed=%v, want %v", ok, tt.want)
			}
			if !tt.want {
				return
			}
			if plan.Repository != tt.repo {
				t.Errorf("repo = %q, want %q", plan.Repository, tt.repo)
			}
			if len(plan.Files) != tt.numFiles {
				t.Errorf("files = %d, want %d", len(plan.Files), tt.numFiles)
			}
			if plan.EmitUrl != tt.emitUrl {
				t.Errorf("EmitUrl = %v, want %v", plan.EmitUrl, tt.emitUrl)
			}
			if plan.EmitRepo != tt.emitRepo {
				t.Errorf("EmitRepo = %v, want %v", plan.EmitRepo, tt.emitRepo)
			}
			for _, f := range plan.Files {
				if !strings.HasPrefix(f, src) {
					t.Errorf("file %q is not rooted at the source dir", f)
				}
			}
		})
	}
}

// Each emitted variable is meaningful to exactly one configuration. Leaking an
// image URL into extensions would replace the OS image with a system
// extension; leaking a sysext repo into misc makes host-config preparation
// hunt for a sysext file that was never built. Both have happened.
func TestEmittedVariablesAreScopedToTheConfigThatNeedsThem(t *testing.T) {
	ext, _ := planFor("extensions", "host", "/src")
	if ext.EmitUrl {
		t.Error("extensions must not emit an image URL")
	}
	misc, _ := planFor("misc", "host", "/src")
	if misc.EmitRepo {
		t.Error("misc must not emit a sysext repository")
	}
}

// misc steps through image versions during its A/B updates, so every version
// must be staged before the run starts; staging only the base image installs
// fine and then fails on the first update, long after the fact.
func TestMiscStagesEveryVersionTheUpdatesStepTo(t *testing.T) {
	plan, ok := planFor("misc", "container", "/src")
	if !ok {
		t.Fatal("misc must push")
	}
	want := []string{"regular.cosi", "regular_v2.cosi", "regular_v3.cosi", "regular_v4.cosi"}
	if len(plan.Files) != len(want) {
		t.Fatalf("staged %d versions, want %d", len(plan.Files), len(want))
	}
	for i, f := range plan.Files {
		if filepath.Base(f) != want[i] {
			t.Errorf("version %d = %q, want %q", i+1, filepath.Base(f), want[i])
		}
	}
}

// The repositories are namespaced away from the legacy suite's, which pushes
// its own differently-built images under the same build id.
func TestRepositoriesAreNamespacedPerRuntime(t *testing.T) {
	host, _ := planFor("extensions", "host", "/src")
	container, _ := planFor("extensions", "container", "/src")
	if host.Repository == container.Repository {
		t.Error("host and container must not share a repository")
	}
	for _, p := range []Plan{host, container} {
		if !strings.Contains(p.Repository, "storm") {
			t.Errorf("repository %q is not namespaced to this suite", p.Repository)
		}
	}
}

func TestTagBaseIdentifiesTheRun(t *testing.T) {
	got := TagBase("1205999", "misc", "virtualMachine")
	if got != "v1205999.misc.virtualMachine" {
		t.Errorf("TagBase = %q", got)
	}
}
