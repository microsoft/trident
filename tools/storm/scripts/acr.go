package scripts

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/sirupsen/logrus"
)

type AcrScriptSet struct {
	// This field represents the "acr-push" subcommand.
	AcrPush AcrPushScript `cmd:"" help:"Pushes images to an ACR"`

	// This field represents the "acr-delete" subcommand.
	AcrDelete AcrDeleteScript `cmd:"" help:"Deletes images from an ACR"`
}

func generateTagBase(buildId string, config string, deploymentEnvironment string) string {
	return fmt.Sprintf("v%s.%s.%s", buildId, config, deploymentEnvironment)
}

func loginToACR(acrName string) error {
	logrus.Infof("Logging in to ACR: %s", acrName)
	cmd := exec.Command("az", "acr", "login", "-n", acrName)
	return cmd.Run()
}

// Define AcrPushScript
type AcrPushScript struct {
	Config                string   `required:"" help:"Trident configuration (e.g., 'extensions')" type:"string"`
	DeploymentEnvironment string   `required:"" help:"Deployment environment (virtualMachine or bareMetal)" type:"string"`
	AcrName               string   `required:"" help:"Azure Container Registry name" type:"string"`
	RepoName              string   `required:"" help:"Repository name in ACR" type:"string"`
	BuildId               string   `required:"" help:"Build ID" type:"string"`
	FilePaths             []string `required:"" help:"Array of file paths to push to ACR"`
}

func (s *AcrPushScript) Run() error {
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

	// Set output variable by writing to stdout
	fmt.Printf("##vso[task.setvariable variable=TAG_BASE]%s\n", tagBase)
	logrus.Infof("TAG_BASE set to: %s", tagBase)

	return nil
}

func (s *AcrPushScript) pushFiles(tagBase string) error {
	for i, filePath := range s.FilePaths {
		// Check if file exists
		if _, err := os.Stat(filePath); os.IsNotExist(err) {
			return fmt.Errorf("file %s does not exist: %w", filePath, err)
		}

		// Create tag with index
		tag := fmt.Sprintf("%s.%d", tagBase, i+1)

		// Push the file
		err := s.pushImage(filePath, tag)
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

	// Sleep to allow registry to process
	time.Sleep(3 * time.Second)

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

// Define AcrDeleteScript
type AcrDeleteScript struct {
	Config                string `required:"" help:"Trident configuration (e.g., 'extensions')" type:"string"`
	DeploymentEnvironment string `required:"" help:"Deployment environment (virtualMachine or bareMetal)" type:"string"`
	AcrName               string `required:"" help:"Azure Container Registry name" type:"string"`
	RepoName              string `required:"" help:"Repository name in ACR" type:"string"`
	BuildId               string `required:"" help:"Build ID" type:"string"`
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
