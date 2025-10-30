package helpers

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"time"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

type PushToACRHelper struct {
	args struct {
		Config                string   `required:"" help:"Trident configuration (e.g., 'extensions')" type:"string"`
		DeploymentEnvironment string   `required:"" help:"Deployment environment (virtualMachine or bareMetal)" type:"string"`
		AcrName               string   `required:"" help:"Azure Container Registry name" type:"string"`
		RepoName              string   `required:"" help:"Repository name in ACR" type:"string"`
		BuildId               string   `required:"" help:"Build ID" type:"string"`
		FilePaths             []string `required:"" help:"Array of file paths to push to ACR"`
	}
}

func (h PushToACRHelper) Name() string {
	return "push-to-acr"
}

func (h *PushToACRHelper) Args() any {
	return &h.args
}

func (h *PushToACRHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("push-to-acr", h.pushToACR)
	return nil
}

func (h *PushToACRHelper) pushToACR(tc storm.TestCase) error {
	// Login to ACR
	err := h.loginToACR()
	if err != nil {
		return fmt.Errorf("failed to login to ACR: %w", err)
	}

	// Push all specified files
	tagBase := fmt.Sprintf("v%s.%s.%s", h.args.BuildId, h.args.Config, h.args.DeploymentEnvironment)
	err = h.pushFiles(tagBase)
	if err != nil {
		return fmt.Errorf("failed to push files: %w", err)
	}

	// Set output variable by writing to stdout
	fmt.Printf("##vso[task.setvariable variable=TAG_BASE]%s\n", tagBase)
	logrus.Infof("TAG_BASE set to: %s\n", tagBase)

	return nil
}

func (h *PushToACRHelper) loginToACR() error {
	logrus.Infof("Logging in to ACR: %s\n", h.args.AcrName)
	cmd := exec.Command("az", "acr", "login", "-n", h.args.AcrName)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}

func (h *PushToACRHelper) pushFiles(tagBase string) error {
	for i, filePath := range h.args.FilePaths {
		// Check if file exists
		if _, err := os.Stat(filePath); os.IsNotExist(err) {
			return fmt.Errorf("file %s does not exist: %w", filePath, err)
		}

		// Create tag with index
		tag := fmt.Sprintf("%s.%d", tagBase, i+1)

		// Push the file
		err := h.pushImage(filePath, tag)
		if err != nil {
			return fmt.Errorf("failed to push file %s: %w", filePath, err)
		}

		// Verify the push
		err = h.verifyImage(h.args.RepoName, tag)
		if err != nil {
			return fmt.Errorf("failed to verify %s:%s: %w", h.args.RepoName, tag, err)
		}
	}

	return nil
}

func (h *PushToACRHelper) pushImage(filePath, tag string) error {
	registryURL := fmt.Sprintf("%s.azurecr.io", h.args.AcrName)
	fullImageName := fmt.Sprintf("%s/%s:%s", registryURL, h.args.RepoName, tag)

	logrus.Infof("Pushing %s with tag %s to %s\n", filePath, tag, registryURL)

	// Get the directory and filename from the full path
	dir := filepath.Dir(filePath)
	fileName := filepath.Base(filePath)

	// Use ORAS to push the image
	cmd := exec.Command("oras", "push", fullImageName, fileName)
	cmd.Dir = dir
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	err := cmd.Run()
	if err != nil {
		return fmt.Errorf("oras push failed for %s: %w", filePath, err)
	}

	// Sleep to allow registry to process
	time.Sleep(3 * time.Second)

	return nil
}

func (h *PushToACRHelper) verifyImage(repository, tag string) error {
	logrus.Infof("Verifying %s:%s was pushed successfully...\n", repository, tag)

	cmd := exec.Command("az", "acr", "repository", "show",
		"--name", h.args.AcrName,
		"--image", fmt.Sprintf("%s:%s", repository, tag))
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	return cmd.Run()
}
