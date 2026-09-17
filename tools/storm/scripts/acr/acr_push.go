package acr

import (
	"fmt"
	"os/exec"
	"path/filepath"
	"time"
	stormretry "tridenttools/storm/utils/retry"

	"github.com/microsoft/storm/pkg/storm/core"
	"github.com/sirupsen/logrus"
)

// Define AcrPushScript
type AcrPushScript struct {
	Config                string `required:"" help:"Trident configuration's name (e.g., 'extensions')"`
	DeploymentEnvironment string `required:"" help:"Deployment environment (virtualMachine or bareMetal)" enum:"virtualMachine,bareMetal"`
	RuntimeEnv            string `required:"" help:"Runtime environment (host or container)"`
	AcrName               string `required:"" help:"Azure Container Registry name"`
	BuildId               string `required:"" help:"Build ID"`
	SourceDir             string `required:"" help:"Trident source directory the artifacts were built into" type:"existingdir"`
	TagVarName            string `required:"" help:"ADO variable name in which to store images' tag base"`
	RepoVarName           string `help:"ADO variable name in which to store images' repo name"`
	UrlVarName            string `help:"ADO variable name in which to store the pushed image's OCI URL"`
}

// maxOciImageVersion is how many versions of the OCI-hosted image are staged.
// It must cover the highest version the scenario can step to, which is the
// split ring's (see maxSplitRingImageVersion in the scenario package); split
// A/B runs from the 'ci' ring upwards.
const maxOciImageVersion = 4

// ociImageName is the on-disk name of version v, matching the convention
// prepare-images uses: v1 is the bare name and later versions carry a _vN
// suffix.
func ociImageName(v int) string {
	if v == 1 {
		return "regular.cosi"
	}
	return fmt.Sprintf("regular_v%d.cosi", v)
}

// pushPlan is what a given configuration needs hosted in ACR.
type pushPlan struct {
	repoName string
	files    []string
	// emitUrl is set only for a configuration that INSTALLS from the pushed
	// image. Emitting it for extensions would point image.url at a sysext
	// image and replace the OS image entirely.
	emitUrl bool
}

// planFor resolves what to push for a configuration, or false when it hosts
// nothing in ACR. This lives here rather than in the pipeline so the step can
// be invoked unconditionally for every configuration: deciding in YAML would
// mean a shell branch that builds an argument list, which is exactly the kind
// of logic this port moves into Go.
//
//   - extensions installs sysext images from ACR.
//   - misc installs its COSI from an OCI registry rather than over HTTP, and is
//     the only coverage of the OCI image source. Ported from the legacy suite's
//     trident-prep.yml, which passed --ociCosiUrl for misc alone.
func (s *AcrPushScript) planFor() (pushPlan, bool) {
	switch s.Config {
	case "extensions":
		return pushPlan{
			repoName: "sysext-storm-" + s.RuntimeEnv,
			files: []string{
				filepath.Join(s.SourceDir, "test-sysext-1.raw"),
				filepath.Join(s.SourceDir, "test-sysext-2.raw"),
			},
			emitUrl: false,
		}, true
	case "misc":
		// Every version the A/B updates will step through must be staged up
		// front. The scenario's prepareTestImages skips versioning for an
		// oci:// URL precisely because the pipeline is expected to have done
		// it, and the OCI branch of runTridentUpdate then bumps the tag
		// suffix (…<N> -> …<N+1>). Pushing only the base image would install
		// fine and then fail on the first update with a missing tag.
		// maxOciImageVersion covers the split ring, which runs from 'ci' up.
		files := make([]string, 0, maxOciImageVersion)
		for v := 1; v <= maxOciImageVersion; v++ {
			files = append(files, filepath.Join(s.SourceDir, "artifacts", "test-image", ociImageName(v)))
		}
		return pushPlan{
			repoName: "cosi-storm-" + s.RuntimeEnv,
			files:    files,
			emitUrl:  true,
		}, true
	default:
		return pushPlan{}, false
	}
}

func (s *AcrPushScript) Run(suite core.SuiteContext) error {
	plan, needed := s.planFor()
	if !needed {
		logrus.Infof("Configuration %q hosts no images in ACR; nothing to push.", s.Config)
		return nil
	}

	// Login to ACR
	err := loginToACR(s.AcrName)
	if err != nil {
		return fmt.Errorf("failed to login to ACR: %w", err)
	}

	// Push all specified files
	tagBase := generateTagBase(s.BuildId, s.Config, s.DeploymentEnvironment)
	err = s.pushFiles(plan, tagBase)
	if err != nil {
		return fmt.Errorf("failed to push files: %w", err)
	}

	if suite.AzureDevops() {
		// Set output variables by writing to stdout
		fmt.Printf("##vso[task.setvariable variable=%s]%s\n", s.TagVarName, tagBase)
		if s.RepoVarName != "" {
			fmt.Printf("##vso[task.setvariable variable=%s]%s\n", s.RepoVarName, plan.repoName)
		}
		if s.UrlVarName != "" && plan.emitUrl {
			// pushFiles tags the first file ".1", matching the URL the
			// scenario will install from.
			fmt.Printf("##vso[task.setvariable variable=%s]%s\n", s.UrlVarName, ociUrl(s.AcrName, plan.repoName, tagBase))
		}
	}
	logrus.Infof("%s set to: %s", s.TagVarName, tagBase)

	return nil
}

func (s *AcrPushScript) pushFiles(plan pushPlan, tagBase string) error {
	for i, filePath := range plan.files {
		// Create tag with index
		tag := fmt.Sprintf("%s.%d", tagBase, i+1)

		// Push the file with retry (5 seconds total until time out; 1 second backoff between attempts)
		_, err := stormretry.Retry(5*time.Second, 1*time.Second, func(attempt int) (*bool, error) {
			err := s.pushImage(plan.repoName, filePath, tag)
			if err != nil {
				return nil, err
			}
			return nil, nil
		})
		if err != nil {
			return fmt.Errorf("failed to push file %s: %w", filePath, err)
		}

		// Verify the push
		err = s.verifyImage(plan.repoName, tag)
		if err != nil {
			return fmt.Errorf("failed to verify %s:%s: %w", plan.repoName, tag, err)
		}
	}

	return nil
}

func (s *AcrPushScript) pushImage(repoName, filePath, tag string) error {
	registryURL := fmt.Sprintf("%s.azurecr.io", s.AcrName)
	fullImageName := fmt.Sprintf("%s/%s:%s", registryURL, repoName, tag)

	logrus.Infof("Pushing %s with tag %s to %s", filePath, tag, registryURL)

	// Get the directory and filename from the full path
	dir := filepath.Dir(filePath)
	fileName := filepath.Base(filePath)

	// Use ORAS to push the image
	cmd := exec.Command("oras", "push", fullImageName, fileName)
	cmd.Dir = dir
	output, err := cmd.CombinedOutput()
	if err != nil {
		logrus.WithField("output", string(output)).Errorf("Failed to push %s with oras", fullImageName)
		return err
	}

	return nil
}

func (s *AcrPushScript) verifyImage(repository, tag string) error {
	logrus.Infof("Verifying %s:%s was pushed successfully...", repository, tag)

	cmd := exec.Command("az", "acr", "repository", "show",
		"--name", s.AcrName,
		"--image", fmt.Sprintf("%s:%s", repository, tag))
	output, err := cmd.CombinedOutput()
	if err != nil {
		logrus.WithField("output", string(output)).Errorf("Failed to verify image %s:%s exists in ACR", repository, tag)
		return err
	}
	return nil
}

// ociUrl renders the registry reference for the first pushed file, which is
// the form Trident consumes as image.url.
func ociUrl(acrName, repoName, tagBase string) string {
	return fmt.Sprintf("oci://%s.azurecr.io/%s:%s.1", acrName, repoName, tagBase)
}
