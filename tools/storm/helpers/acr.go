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

type AcrHelper struct {
	args struct {
		Push                  bool     `required:"" help:"'true' if AcrHelper should push images to the ACR; 'false' if AcrHelper should remove images from ACR" type:"bool"`
		Config                string   `required:"" help:"Trident configuration (e.g., 'extensions')" type:"string"`
		DeploymentEnvironment string   `required:"" help:"Deployment environment (virtualMachine or bareMetal)" type:"string"`
		AcrName               string   `required:"" help:"Azure Container Registry name" type:"string"`
		RepoName              string   `required:"" help:"Repository name in ACR" type:"string"`
		BuildId               string   `required:"" help:"Build ID" type:"string"`
		FilePaths             []string `help:"Array of file paths to push to ACR"`
		NumClones             int      `help:"Number of copies of file to remove from ACR repository" type:"int"`
	}
}

func (h AcrHelper) Name() string {
	return "acr"
}

func (h *AcrHelper) Args() any {
	return &h.args
}

func (h *AcrHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("push-to-acr", h.pushToACR)
	r.RegisterTestCase("remove-from-acr", h.removeFromAcr)
	return nil
}

func (h *AcrHelper) pushToACR(tc storm.TestCase) error {
	if !h.args.Push {
		tc.Skip("Push to ACR not requested.")
	}

	// Login to ACR
	err := h.loginToACR()
	if err != nil {
		return fmt.Errorf("failed to login to ACR: %w", err)
	}

	// Push all specified files
	tagBase := h.generateTagBase()
	err = h.pushFiles(tagBase)
	if err != nil {
		return fmt.Errorf("failed to push files: %w", err)
	}

	// Set output variable by writing to stdout
	fmt.Printf("##vso[task.setvariable variable=TAG_BASE]%s\n", tagBase)
	logrus.Infof("TAG_BASE set to: %s\n", tagBase)

	return nil
}

func (h *AcrHelper) removeFromAcr(tc storm.TestCase) error {
	if h.args.Push {
		tc.Skip("Remove from ACR not requested.")
	}

	// Login to ACR
	err := h.loginToACR()
	if err != nil {
		return fmt.Errorf("failed to login to ACR: %w", err)
	}

	tagBase := h.generateTagBase()
	// Delete COSI images (for misc config)
	h.deleteImagesWithTagBase(tagBase)

	logrus.Infof("Successfully completed ACR cleanup")
	return nil
}

func (h *AcrHelper) generateTagBase() string {
	return fmt.Sprintf("v%s.%s.%s", h.args.BuildId, h.args.Config, h.args.DeploymentEnvironment)
}

func (h *AcrHelper) loginToACR() error {
	logrus.Infof("Logging in to ACR: %s\n", h.args.AcrName)
	cmd := exec.Command("az", "acr", "login", "-n", h.args.AcrName)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}

func (h *AcrHelper) pushFiles(tagBase string) error {
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

func (h *AcrHelper) pushImage(filePath, tag string) error {
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

func (h *AcrHelper) verifyImage(repository, tag string) error {
	logrus.Infof("Verifying %s:%s was pushed successfully...\n", repository, tag)

	cmd := exec.Command("az", "acr", "repository", "show",
		"--name", h.args.AcrName,
		"--image", fmt.Sprintf("%s:%s", repository, tag))
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	return cmd.Run()
}

func (h *AcrHelper) deleteImagesWithTagBase(tagBase string) {
	logrus.Infof("Deleting images from repository %s with tag base %s", h.args.RepoName, tagBase)

	for i := 1; i <= h.args.NumClones; i++ {
		tag := fmt.Sprintf("%s.%d", tagBase, i)
		err := h.deleteImageIfExists(h.args.RepoName, tag)
		if err != nil {
			logrus.Warnf("Failed to delete %s:%s: %v", h.args.RepoName, tag, err)
			// Continue with other images even if one fails
		}
	}
}

func (h *AcrHelper) deleteImageIfExists(repository, tag string) error {
	// First check if the image exists
	imageName := fmt.Sprintf("%s:%s", repository, tag)
	checkCmd := exec.Command("az", "acr", "repository", "show",
		"--name", h.args.AcrName,
		"--image", imageName)
	checkCmd.Stdout = os.Stdout
	checkCmd.Stderr = os.Stderr
	err := checkCmd.Run()
	if err != nil {
		// Image doesn't exist, skip deletion
		logrus.Debugf("Image %s/%s does not exist, skipping deletion", h.args.AcrName, imageName)
		return nil
	}

	// Image exists, delete it
	logrus.Infof("Deleting image: %s", imageName)
	deleteCmd := exec.Command("az", "acr", "repository", "delete",
		"--name", h.args.AcrName,
		"--image", imageName,
		"--yes")
	deleteCmd.Stdout = os.Stdout
	deleteCmd.Stderr = os.Stderr

	return deleteCmd.Run()
}
