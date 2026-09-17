package acr

import (
	"fmt"
	"os/exec"
	"strings"

	"github.com/sirupsen/logrus"
)

// Define AcrDeleteScript
type AcrDeleteScript struct {
	Config                string `required:"" help:"Trident configuration's name (e.g., 'extensions')"`
	DeploymentEnvironment string `required:"" help:"Deployment environment (virtualMachine or bareMetal)" enum:"virtualMachine,bareMetal"`
	AcrName               string `required:"" help:"Azure Container Registry name"`
	BuildId               string `required:"" help:"Build ID"`

	// Explicit mode, used by the legacy pipeline: it names the repository and
	// how many tags to remove. Both must be given together.
	RepoName  string `help:"Repository name in ACR. Omit to derive it from the configuration."`
	NumClones int    `help:"Number of tags to delete. Omit to derive from the configuration." type:"int"`

	// Derived mode: resolve the same plan acr-push used, so cleanup cannot
	// drift from the push.
	RuntimeEnv string `help:"Runtime environment (host or container), used to derive the repository name"`
	SourceDir  string `help:"Trident source directory the artifacts were built into"`
}

func (s *AcrDeleteScript) Run() error {
	// Resolve what this configuration pushed from the same place acr-push
	// does, so cleanup cannot drift from the push and leak images.
	push := AcrPushScript{
		Config:     s.Config,
		RuntimeEnv: s.RuntimeEnv,
		SourceDir:  s.SourceDir,
		RepoName:   s.RepoName,
		FilePaths:  make([]string, s.NumClones),
	}
	plan, pushed := push.planFor()
	if !pushed {
		logrus.Infof("Configuration %q hosts no images in ACR; nothing to clean up.", s.Config)
		return nil
	}

	// Login to ACR
	err := loginToACR(s.AcrName)
	if err != nil {
		return fmt.Errorf("failed to login to ACR: %w", err)
	}

	tagBase := generateTagBase(s.BuildId, s.Config, s.DeploymentEnvironment)
	s.deleteImagesWithTagBase(plan, tagBase)

	logrus.Infof("Successfully completed ACR cleanup")
	return nil
}

func (s *AcrDeleteScript) deleteImagesWithTagBase(plan pushPlan, tagBase string) {
	logrus.Infof("Deleting images from repository %s with tag base %s", plan.repoName, tagBase)

	// One tag per pushed file, numbered the way pushFiles numbered them.
	for i := range plan.files {
		tag := fmt.Sprintf("%s.%d", tagBase, i+1)
		s.deleteImageIfExists(plan.repoName, tag)
	}
}

func (s *AcrDeleteScript) deleteImageIfExists(repository, tag string) {
	// First check if the image exists
	imageName := fmt.Sprintf("%s:%s", repository, tag)
	checkCmd := exec.Command("az", "acr", "repository", "show",
		"--name", s.AcrName,
		"--image", imageName)
	logrus.Debugf("Executing command: %s %s", checkCmd.Path, strings.Join(checkCmd.Args[1:], " "))
	output, err := checkCmd.CombinedOutput()
	if err != nil {
		logrus.WithField("output", string(output)).Errorf("Image %s does not exist: %v, skipping deletion", imageName, err)
		return
	}

	// Image exists, delete it
	logrus.Infof("Deleting image: %s", imageName)
	deleteCmd := exec.Command("az", "acr", "repository", "delete",
		"--name", s.AcrName,
		"--image", imageName,
		"--yes")
	logrus.Debugf("Executing command: %s %s", deleteCmd.Path, strings.Join(deleteCmd.Args[1:], " "))
	output, err = deleteCmd.CombinedOutput()
	if err != nil {
		logrus.WithField("output", string(output)).Errorf("Failed to delete %s: %v", imageName, err)
	}
}
