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
	DeploymentEnvironment string `required:"" help:"Deployment environment (virtualMachine or bareMetal)"`
	AcrName               string `required:"" help:"Azure Container Registry name"`
	RepoName              string `required:"" help:"Repository name in ACR"`
	BuildId               string `required:"" help:"Build ID"`
	NumClones             int    `required:"" help:"Number of copies of file to delete from ACR repository" type:"int"`
}

func (s *AcrDeleteScript) Run() error {
	// Login to ACR
	err := loginToACR(s.AcrName)
	if err != nil {
		return fmt.Errorf("failed to login to ACR: %w", err)
	}

	tagBase := generateTagBase(s.BuildId, s.Config, s.DeploymentEnvironment)
	// Delete COSI images (for misc config)
	s.deleteImagesWithTagBase(tagBase)

	logrus.Infof("Successfully completed ACR cleanup")
	return nil
}

func (s *AcrDeleteScript) deleteImagesWithTagBase(tagBase string) {
	logrus.Infof("Deleting images from repository %s with tag base %s", s.RepoName, tagBase)

	for i := 1; i <= s.NumClones; i++ {
		tag := fmt.Sprintf("%s.%d", tagBase, i)
		s.deleteImageIfExists(s.RepoName, tag)
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
