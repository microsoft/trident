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
		{config: "misc", want: true, repo: "cosi-storm-host", numFiles: 4, emitUrl: true},
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

// Each ADO variable this script sets is meaningful to exactly one
// configuration, and leaking one into another has already caused two distinct
// failures: an OCI URL emitted for extensions would replace the OS image with a
// sysext image, and a sysext repo emitted for misc made prepare-hc try to hash
// a sysext file that only extensions builds. Pin the whole matrix.
func TestVariableEmissionIsScopedToTheConfigThatNeedsIt(t *testing.T) {
	tests := []struct {
		config   string
		emitUrl  bool
		emitRepo bool
	}{
		{"extensions", false, true},
		{"misc", true, false},
	}
	for _, tt := range tests {
		t.Run(tt.config, func(t *testing.T) {
			s := AcrPushScript{Config: tt.config, RuntimeEnv: "host", SourceDir: "/src"}
			plan, ok := s.planFor()
			if !ok {
				t.Fatalf("%s must push", tt.config)
			}
			if plan.emitUrl != tt.emitUrl {
				t.Errorf("emitUrl = %v, want %v", plan.emitUrl, tt.emitUrl)
			}
			if plan.emitRepo != tt.emitRepo {
				t.Errorf("emitRepo = %v, want %v", plan.emitRepo, tt.emitRepo)
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

// Cleanup must delete exactly what the push created, including the misc COSI.
// Deriving both from planFor is what keeps them from drifting: the previous
// cleanup hard-coded the sysext repo, so a misc push would have leaked.
func TestCleanupCoversEveryPushedImage(t *testing.T) {
	for _, cfg := range []string{"extensions", "misc", "base"} {
		push := AcrPushScript{Config: cfg, RuntimeEnv: "host", SourceDir: "/src"}
		pushPlan, pushes := push.planFor()

		del := AcrDeleteScript{Config: cfg, RuntimeEnv: "host", SourceDir: "/src"}
		delAsPush := AcrPushScript{Config: del.Config, RuntimeEnv: del.RuntimeEnv, SourceDir: del.SourceDir}
		delPlan, deletes := delAsPush.planFor()

		if pushes != deletes {
			t.Errorf("%s: push=%v but cleanup=%v", cfg, pushes, deletes)
		}
		if !pushes {
			continue
		}
		if pushPlan.repoName != delPlan.repoName {
			t.Errorf("%s: pushed to %q but cleanup targets %q", cfg, pushPlan.repoName, delPlan.repoName)
		}
		if len(pushPlan.files) != len(delPlan.files) {
			t.Errorf("%s: pushed %d image(s) but cleanup deletes %d", cfg, len(pushPlan.files), len(delPlan.files))
		}
	}
}

// misc runs A/B updates, and the OCI branch bumps the tag suffix rather than
// renaming a file. Every version the scenario can step to must therefore exist
// in ACR before the run starts: staging only the base image installs fine and
// then fails on the first update with a missing tag, ~20 minutes in.
func TestMiscStagesEveryImageVersionTheUpdatesWillStepTo(t *testing.T) {
	s := AcrPushScript{Config: "misc", RuntimeEnv: "host", SourceDir: "/src"}
	plan, ok := s.planFor()
	if !ok {
		t.Fatal("misc must push")
	}

	// maxSplitRingImageVersion in the scenario package is 4, and split A/B
	// runs from the ci ring upwards, so all four must be staged.
	if len(plan.files) != 4 {
		t.Fatalf("staged %d versions, want 4", len(plan.files))
	}

	want := []string{"regular.cosi", "regular_v2.cosi", "regular_v3.cosi", "regular_v4.cosi"}
	for i, f := range plan.files {
		if filepath.Base(f) != want[i] {
			t.Errorf("version %d = %q, want %q", i+1, filepath.Base(f), want[i])
		}
	}
}

// The legacy pipeline still runs alongside this suite and invokes these same
// scripts with the flags it has always used (push-to-acr.yml passes
// --repo-name and --file-paths; remove-from-acr.yml passes --repo-name and
// --num-clones). Deriving the plan must not break those callers.
func TestLegacyExplicitInvocationStillWorks(t *testing.T) {
	legacyFiles := []string{
		"/src/artifacts/test-image/regular.cosi",
		"/src/artifacts/test-image/regular_v2.cosi",
		"/src/artifacts/test-image/regular_v3.cosi",
		"/src/artifacts/test-image/regular_v4.cosi",
	}

	// Legacy push: repository and files named explicitly, no runtime/source dir.
	push := AcrPushScript{Config: "misc", RepoName: "trident-testimage", FilePaths: legacyFiles}
	plan, ok := push.planFor()
	if !ok {
		t.Fatal("explicit invocation must push")
	}
	if plan.repoName != "trident-testimage" {
		t.Errorf("repo = %q, want the explicitly named one", plan.repoName)
	}
	if len(plan.files) != len(legacyFiles) {
		t.Errorf("files = %d, want %d", len(plan.files), len(legacyFiles))
	}

	// A configuration legacy pushes but storm's plan does not know about must
	// still work when named explicitly.
	other := AcrPushScript{Config: "base", RepoName: "some-repo", FilePaths: legacyFiles[:1]}
	if _, ok := other.planFor(); !ok {
		t.Error("explicit invocation must push regardless of configuration")
	}

	// Legacy delete: --repo-name plus --num-clones, no source dir.
	del := AcrDeleteScript{Config: "misc", RepoName: "trident-testimage", NumClones: 4}
	delAsPush := AcrPushScript{
		Config: del.Config, RepoName: del.RepoName, FilePaths: make([]string, del.NumClones),
	}
	delPlan, ok := delAsPush.planFor()
	if !ok {
		t.Fatal("explicit delete must resolve a plan")
	}
	if delPlan.repoName != "trident-testimage" || len(delPlan.files) != 4 {
		t.Errorf("delete plan = %q/%d, want trident-testimage/4", delPlan.repoName, len(delPlan.files))
	}
}

// The explicit arguments are a pair. Accepting half of one would silently fall
// through to the derived plan and act on a repository the caller never named --
// for cleanup, that means deleting a different repository's images.
func TestPartialExplicitInputIsRejected(t *testing.T) {
	tests := []struct {
		name  string
		repo  string
		count int
		valid bool
	}{
		{"neither: derive the plan", "", 0, true},
		{"both: explicit", "some-repo", 4, true},
		{"repo without images", "some-repo", 0, false},
		{"images without repo", "", 4, false},
		{"negative count", "some-repo", -1, false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := validateExplicit(tt.repo, tt.count)
			if tt.valid && err != nil {
				t.Errorf("rejected a valid combination: %v", err)
			}
			if !tt.valid && err == nil {
				t.Error("accepted an invalid combination")
			}
		})
	}
}

// A negative count previously reached make([]string, n) and panicked.
func TestNegativeCountDoesNotPanic(t *testing.T) {
	del := AcrDeleteScript{Config: "misc", RepoName: "r", NumClones: -1}
	if err := validateExplicit(del.RepoName, del.NumClones); err == nil {
		t.Fatal("a negative image count must be rejected before it reaches make()")
	}
}
