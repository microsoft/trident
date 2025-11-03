package acr

import (
	"fmt"
	"os/exec"
	"path/filepath"
	"time"
	"tridenttools/storm/utils"

	"github.com/microsoft/storm/pkg/storm/core"
	"github.com/sirupsen/logrus"
)

// Define AcrPushScript
type AcrPushScript struct {
	Config                string   `required:"" help:"Trident configuration's name (e.g., 'extensions')" enum:"misc,extensions"`
	DeploymentEnvironment string   `required:"" help:"Deployment environment (virtualMachine or bareMetal)" enum:"virtualMachine,bareMetal"`
	AcrName               string   `required:"" help:"Azure Container Registry name"`
	RepoName              string   `required:"" help:"Repository name in ACR"`
	BuildId               string   `required:"" help:"Build ID"`
	FilePaths             []string `required:"" help:"Array of file paths to push to ACR" type:"existingfile"`
	TagVarName            string   `required:"" help:"ADO variable name in which to store images' tag base"`
}

func (s *AcrPushScript) Run(suite core.SuiteContext) error {
	// Login to ACR
	err := loginToACR(s.AcrName)
	if err != nil {
		return fmt.Errorf("failed to login to ACR: %w", err)
	}

	// Push all specified files
	tagBase := generateTagBase(s.BuildId, s.Config, s.DeploymentEnvironment)
	err = s.pushFiles(tagBase)
	if err != nil {
		return fmt.Errorf("failed to push files: %w", err)
	}

	if suite.AzureDevops() {
		// Set output variable by writing to stdout
		fmt.Printf("##vso[task.setvariable variable=%s]%s\n", s.TagVarName, tagBase)
	}
	logrus.Infof("%s set to: %s", s.TagVarName, tagBase)

	return nil
}

func (s *AcrPushScript) pushFiles(tagBase string) error {
	for i, filePath := range s.FilePaths {
		// Create tag with index
		tag := fmt.Sprintf("%s.%d", tagBase, i+1)

		// Push the file with retry (5 seconds total until time out; 1 second backoff between attempts)
		_, err := utils.Retry(5*time.Second, 1*time.Second, func(attempt int) (*bool, error) {
			err := s.pushImage(filePath, tag)
			if err != nil {
				return nil, err
			}
			return nil, nil
		})
		if err != nil {
			return fmt.Errorf("failed to push file %s: %w", filePath, err)
		}

		// Verify the push
		err = s.verifyImage(s.RepoName, tag)
		if err != nil {
			return fmt.Errorf("failed to verify %s:%s: %w", s.RepoName, tag, err)
		}
	}

	return nil
}

func (s *AcrPushScript) pushImage(filePath, tag string) error {
	registryURL := fmt.Sprintf("%s.azurecr.io", s.AcrName)
	fullImageName := fmt.Sprintf("%s/%s:%s", registryURL, s.RepoName, tag)

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
