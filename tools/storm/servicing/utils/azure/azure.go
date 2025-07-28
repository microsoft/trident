// Package storm provides helpers for Trident loop-update Storm tests.
// This file contains helpers converted from Bash scripts in scripts/loop-update.
package azure

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/sirupsen/logrus"
)

type AzureConfig struct {
	Subscription                string `help:"Azure subscription" default:"b8a0db63-c5fa-4198-8e2a-f9d6ff52465e"`
	StorageAccountResourceGroup string `help:"Azure resource group" default:"azlinux_bmp_dev"`
	StorageAccount              string `help:"Azure storage account for VM artifacts" default:"azlinuxbmpdev"`
	StorageContainerName        string `help:"Azure storage continer for VM artifacts" default:""`
	WhoAmI                      string `help:"User who is running the tests, used for tagging resources" default:""`
	Region                      string `help:"Azure region" default:"eastus2"`
	SubnetId                    string `help:"Azure subnet ID" default:"/subscriptions/04cdc145-a4f9-42d4-9868-c46d23d0c63f/resourceGroups/trident-vm_servicing-azure-vnet/providers/Microsoft.Network/virtualNetworks/poolpeeringvnet/subnets/default"`
	SshPublicKeyPath            string `help:"Path to SSH public key" default:"~/.ssh/id_rsa.pub"`
	GalleryName                 string `help:"Azure Shared Image Gallery name" default:""`
	GalleryResourceGroup        string `help:"Azure Shared Image Gallery resource group" default:""`
	ImageDefinition             string `help:"Azure Shared Image Gallery image definition" default:"trident-vm-grub-verity-azure-testimage"`
	Offer                       string `help:"Azure offer for the VM" default:"trident-vm-grub-verity-azure-offer"`
	Size                        string `help:"Azure VM size" default:"Standard_D2ds_v5"`
	TestResourceGroup           string `help:"Azure resource group for the VM" default:""`
}

func (cfg AzureConfig) GetStorageAccountUrl() string {
	return fmt.Sprintf("https://%s.blob.core.windows.net", cfg.StorageAccount)
}

func (cfg AzureConfig) GetStorageAccountId() string {
	return fmt.Sprintf("/subscriptions/%s/resourceGroups/%s/providers/Microsoft.Storage/storageAccounts/%s", cfg.Subscription, cfg.StorageAccountResourceGroup, cfg.StorageAccount)
}

func (cfg AzureConfig) ImageVersionExists(imageVersion string) bool {
	_, err := cfg.CallAzCli(
		[]string{
			"sig", "image-version", "show",
			"--resource-group", cfg.GetGalleryResourceGroup(),
			"--gallery-name", cfg.GetGalleryName(),
			"--gallery-image-definition", cfg.ImageDefinition,
			"--gallery-image-version", imageVersion,
		},
		false,
	)
	return err == nil
}

func (cfg AzureConfig) CreateImageVersion(imageVersion string, storageAccountResourceId string, storageBlobEndpoint string) error {
	logrus.Tracef("Create image version in Azure Shared Image Gallery")
	output, err := cfg.CallAzCli(
		[]string{
			"sig", "image-version", "create",
			"--resource-group", cfg.GetGalleryResourceGroup(),
			"--gallery-name", cfg.GetGalleryName(),
			"--gallery-image-definition", cfg.ImageDefinition,
			"--gallery-image-version", imageVersion,
			"--target-regions", cfg.Region,
			"--location", cfg.Region,
			"--replication-mode", "Shallow",
			"--os-vhd-storage-account", storageAccountResourceId,
			"--os-vhd-uri", storageBlobEndpoint,
		},
		true,
	)
	if err != nil {
		logrus.Tracef("Failed to create image version in Azure SIG (%v): %s", err, output)
		return fmt.Errorf("failed to create image version in Azure SIG: %w", err)
	}

	return nil
}

func (cfg AzureConfig) DeployAzureVM(vmName string, user string, buildId string) error {
	if err := cfg.SetSubscription(); err != nil {
		return fmt.Errorf("failed to set Azure subscription: %w", err)
	}

	err := cfg.EnsureGroupExists(cfg.GetTestResourceGroup(), true)
	if err != nil {
		return fmt.Errorf("failed to create Azure resource group: %w", err)
	}

	if cfg.SubnetId != "" {
		// Loop until subnet resource is available
		for {
			vnetResourceArgs := []string{
				"resource", "show",
				"--ids", cfg.SubnetId,
			}
			out, err := cfg.CallAzCli(vnetResourceArgs, true)
			if err == nil {
				// Subnet found, continue with test
				logrus.Tracef("Subnet (%v) found to be available: %v", cfg.SubnetId, out)
				break
			}
			// Log and retry
			logrus.Tracef("Waiting for subnet (%v) to be available: %v", cfg.SubnetId, out)
			time.Sleep(time.Second)
		}
	}

	imageVersion := cfg.GetImageVersion(buildId, false)

	// Create the VM
	vmCreateArgs := []string{
		"vm", "create",
		"--resource-group", cfg.GetTestResourceGroup(),
		"--name", vmName,
		"--size", cfg.Size,
		"--os-disk-size-gb", "60",
		"--admin-username", user,
		"--ssh-key-values", cfg.SshPublicKeyPath,
		"--image", fmt.Sprintf("/subscriptions/%s/resourceGroups/%s/providers/Microsoft.Compute/galleries/%s/images/%s/versions/%s",
			cfg.Subscription, cfg.GetGalleryResourceGroup(), cfg.GetGalleryName(), cfg.ImageDefinition, imageVersion),
		"--location", cfg.Region,
		"--security-type", "TrustedLaunch",
		"--enable-secure-boot", "true",
		"--enable-vtpm", "true",
		"--no-wait",
	}
	if cfg.SubnetId != "" {
		vmCreateArgs = append(vmCreateArgs, "--subnet", cfg.SubnetId)
	}
	logrus.Tracef("Creating Azure VM with args: %v", vmCreateArgs)

	createVmOutput, err := cfg.CallAzCli(vmCreateArgs, true)
	logrus.Tracef("Azure VM creation (%v): %s", err, createVmOutput)
	if err != nil {
		return fmt.Errorf("failed to create Azure VM (%w):\n%s", err, createVmOutput)
	}

	for {
		out, err := cfg.CallAzCli(
			[]string{"vm", "boot-diagnostics", "enable", "--name", vmName, "--resource-group", cfg.GetTestResourceGroup()},
			true,
		)
		if err == nil {
			break
		}
		logrus.Tracef("Failed to enable boot diagnostics for VM '%s': %s", vmName, out)
		time.Sleep(1 * time.Second) // Retry after a short delay
	}

	for {
		out, err := cfg.CallAzCli(
			[]string{"vm", "boot-diagnostics", "get-boot-log", "--name", vmName, "--resource-group", cfg.GetTestResourceGroup()},
			true,
		)
		if err == nil {
			if strings.Contains(string(out), "BlobNotFound") {
				time.Sleep(5 * time.Second) // Wait before retrying
				continue                    // Retry until the boot log is available
			}
			break
		}
		logrus.Tracef("Failed to get boot diagnostics log for VM '%s': %s", vmName, out)
		time.Sleep(5 * time.Second) // Retry after a short delay
	}

	for {
		out, err := cfg.CallAzCli(
			[]string{"vm", "show", "-d", "-g", cfg.GetTestResourceGroup(), "-n", vmName, "--query", "provisioningState", "-o", "tsv"},
			false,
		)
		if err != nil || strings.TrimSpace(string(out)) != "Succeeded" {
			logrus.Tracef("VM '%s' provisioning state is not 'Succeeded': %s", vmName, out)
			time.Sleep(1 * time.Second) // Wait before retrying
			continue                    // Retry until the VM is successfully provisioned
		}
		break
	}
	return nil
}

func (cfg AzureConfig) CleanupAzureVM() error {
	if err := cfg.SetSubscription(); err != nil {
		return fmt.Errorf("failed to set Azure subscription: %w", err)
	}
	if err := cfg.DeleteGroup(cfg.GetTestResourceGroup()); err != nil {
		return fmt.Errorf("failed to delete Azure resource group: %w", err)
	}
	return nil
}

func (cfg AzureConfig) PublishSigImage(artifactsDir string, buildId string) error {
	if err := cfg.SetSubscription(); err != nil {
		return fmt.Errorf("failed to set Azure subscription: %w", err)
	}

	imageVersion := cfg.GetImageVersion(buildId, true)
	logrus.Infof("Using image version %s", imageVersion)

	if err := cfg.checkImageVersionExists(imageVersion); err == nil {
		logrus.Infof("Image version %s already exists. Exiting...", imageVersion)
		return nil // Image version already exists, no need to proceed
	}
	logrus.Tracef("Image version %s does not exist", imageVersion)

	return cfg.publishSigImageVersion(artifactsDir, imageVersion)
}

func (cfg AzureConfig) checkImageVersionExists(imageVersion string) error {
	logrus.Tracef("Check if image version %s already exists", imageVersion)
	_, err := cfg.CallAzCli(
		[]string{
			"sig", "image-version", "show",
			"--resource-group", cfg.GetGalleryResourceGroup(),
			"--gallery-name", cfg.GetGalleryName(),
			"--gallery-image-definition", cfg.ImageDefinition,
			"--gallery-image-version", imageVersion},
		false,
	)
	if err != nil {
		logrus.Tracef("Image version %s does not exist", imageVersion)
		return err
	}
	logrus.Infof("Image version %s already exists. Exiting...", imageVersion)
	return nil
}

func (cfg AzureConfig) publishSigImageVersion(artifactsDir string, imageVersion string) error {
	now := time.Now()
	currentDate := now.Format("20060102")
	currentTime := now.Format("150405")

	storageAccountUrl := cfg.GetStorageAccountUrl()
	storageAccountResourceId := cfg.GetStorageAccountId()
	storageContainerName := cfg.GetStorageContainerName()
	imagePath := filepath.Join(artifactsDir, "trident-vm-grub-verity-azure-testimage.vhd")

	logrus.Tracef("Prepare image for Azure Shared Image Gallery")
	err := cfg.PrepareSigImage()
	if err != nil {
		return fmt.Errorf("failed to prepare image for Azure: %w", err)
	}

	storageBlobName := fmt.Sprintf("%s.%s-%s.vhd", currentDate, currentTime, imageVersion)
	storageBlobEndpoint := fmt.Sprintf("%s/%s/%s", storageAccountUrl, storageContainerName, storageBlobName)

	// Get the path to the VHD file
	logrus.Tracef("Resize VHD image for upload")
	if err := cfg.ResizeImage(artifactsDir, imagePath); err != nil {
		return fmt.Errorf("failed to resize image: %w", err)
	}

	logrus.Tracef("Ensure azcopy is installed")
	if err := cfg.EnsureAzcopyExists(); err != nil {
		return fmt.Errorf("failed to ensure azcopy exists: %w", err)
	}

	// Upload the image artifact to Steamboat Storage Account
	logrus.Tracef("Upload image to Azure Storage Blob: %s", storageBlobEndpoint)
	if azcopyOutput, err := exec.Command("azcopy", "copy", imagePath, storageBlobEndpoint).CombinedOutput(); err != nil {
		logrus.Tracef("Failed to upload image to Azure Storage Blob (%v): %s", err, azcopyOutput)
		return fmt.Errorf("failed to upload image to Azure Storage: %w", err)
	}

	logrus.Tracef("Create image version in Azure Shared Image Gallery")
	createImageVersionOutput, err := cfg.CallAzCli(
		[]string{
			"sig", "image-version", "create",
			"--resource-group", cfg.GetGalleryResourceGroup(),
			"--gallery-name", cfg.GetGalleryName(),
			"--gallery-image-definition", cfg.ImageDefinition,
			"--gallery-image-version", imageVersion,
			"--target-regions", cfg.Region,
			"--location", cfg.Region,
			"--replication-mode", "Shallow",
			"--os-vhd-storage-account", storageAccountResourceId,
			"--os-vhd-uri", storageBlobEndpoint,
		},
		true,
	)
	if err != nil {
		logrus.Tracef("Failed to create image version in Azure SIG (%v): %s", err, createImageVersionOutput)
		return fmt.Errorf("failed to create image version in Azure SIG: %w", err)
	}

	return nil
}

func (cfg AzureConfig) EnsureAzcopyExists() error {
	// if ! which azcopy; then
	if _, err := exec.LookPath("azcopy"); err != nil {
		logrus.Info("azcopy not found, installing...")
		// 	Install az-copy dependency
		pipelineAgentOs, err := os.ReadFile("/etc/os-release")
		if err != nil {
			return fmt.Errorf("failed to read /etc/os-release: %w", err)
		}
		pipelineAgentOsId := ""
		for _, lines := range strings.Split(string(pipelineAgentOs), "\n") {
			if strings.HasPrefix(lines, "ID=") {
				pipelineAgentOsId = strings.Trim(strings.Split(lines, "=")[1], "\"")
				break
			}
		}
		if pipelineAgentOsId == "" {
			return fmt.Errorf("failed to determine OS ID from /etc/os-release")
		}

		pipelineAgentOsVersion := ""
		for _, lines := range strings.Split(string(pipelineAgentOs), "\n") {
			if strings.HasPrefix(lines, "VERSION_ID=") {
				pipelineAgentOsVersion = strings.Trim(strings.Split(lines, "=")[1], "\"")
				break
			}
		}
		if pipelineAgentOsVersion == "" {
			return fmt.Errorf("failed to determine OS version from /etc/os-release")
		}

		azcopyDownloadUrl := fmt.Sprintf("https://packages.microsoft.com/config/%s/%s/packages-microsoft-prod.deb", pipelineAgentOsId, pipelineAgentOsVersion)
		if err := exec.Command("curl", "-sSL", "-O", azcopyDownloadUrl).Run(); err != nil {
			logrus.Errorf("Failed to download the debian package repo while attempting to install azcopy: %v", err)
			logrus.Error("Suggestion: Are you using a new, non-ubuntu, pipeline agent? If yes, add azcopy installation logic for the new build agent.")
			return fmt.Errorf("failed to download the debian package repo while attempting to install azcopy: %w", err)
		}

		if err := exec.Command("sudo", "dpkg", "-i", "packages-microsoft-prod.deb").Run(); err != nil {
			return fmt.Errorf("failed to install debian package while attempting to install azcopy: %w", err)
		}
		if err := os.Remove("packages-microsoft-prod.deb"); err != nil {
			return fmt.Errorf("failed to remove debian package file: %w", err)
		}
		if err := exec.Command("sudo", "apt-get", "update", "-y").Run(); err != nil {
			return fmt.Errorf("failed to update package list while attempting to install azcopy: %w", err)
		}
		if err := exec.Command("sudo", "apt-get", "install", "azcopy", "-y").Run(); err != nil {
			return fmt.Errorf("failed to install azcopy: %w", err)
		}
		out, err := exec.Command("azcopy", "--version").Output()
		if err != nil {
			return fmt.Errorf("failed to check azcopy version: %w", err)
		}
		logrus.Infof("azcopy version: %s", strings.TrimSpace(string(out)))
	}
	return nil
}

func (cfg AzureConfig) ResizeImage(artifactsDir string, imagePath string) error {
	// VHD images on Azure must have a virtual size aligned to 1MB. https://learn.microsoft.com/en-us/azure/virtual-machines/linux/create-upload-generic#resize-vhds
	MB := 1024 * 1024

	rawFile := filepath.Join(artifactsDir, "resize.raw")
	// Convert to raw format
	if out, err := exec.Command("sudo", "qemu-img", "convert", "-f", "vpc", "-O", "raw", imagePath, rawFile).CombinedOutput(); err != nil {
		logrus.Tracef("Failed to convert VHD to raw: %v\n%s", err, out)
		return fmt.Errorf("failed to convert VHD to raw: %w", err)
	}

	// Get the size of the raw image
	out, err := exec.Command("qemu-img", "info", "-f", "raw", "--output", "json", rawFile).Output()
	if err != nil {
		return fmt.Errorf("failed to get raw image size: %w", err)
	}
	re := regexp.MustCompile(`"virtual-size":\s*([0-9]+)`)
	matches := re.FindSubmatch(out)
	if len(matches) < 2 {
		return fmt.Errorf("failed to parse raw image size from output: %s", out)
	}
	size, err := strconv.Atoi(string(matches[1]))
	if err != nil {
		return fmt.Errorf("failed to convert raw image size to integer: %w", err)
	}

	roundedSize := ((size + MB - 1) / MB) * MB
	logrus.Infof("Rounded Size = %d", roundedSize)

	// Resize the raw image to the rounded size
	if out, err := exec.Command("sudo", "qemu-img", "resize", rawFile, fmt.Sprintf("%d", roundedSize)).CombinedOutput(); err != nil {
		logrus.Tracef("Failed to resize raw image: %v\n%s", err, out)
		return fmt.Errorf("failed to resize raw image: %w", err)
	}
	// Convert back to original format
	if out, err := exec.Command("sudo", "qemu-img", "convert", "-f", "raw", "-o", "subformat=fixed,force_size", "-O", "vpc", rawFile, imagePath).CombinedOutput(); err != nil {
		logrus.Tracef("Failed to convert raw back to VHD: %v\n%s", err, out)
		return fmt.Errorf("failed to convert raw back to VHD: %w", err)
	}
	// Remove the temporary raw file
	if err := exec.Command("rm", rawFile).Run(); err != nil {
		return fmt.Errorf("failed to remove temporary raw file: %w", err)
	}
	return nil
}

func (cfg AzureConfig) PrepareSigImage() error {
	logrus.Tracef("Set Azure subscription")
	if err := cfg.SetSubscription(); err != nil {
		return fmt.Errorf("failed to set Azure subscription: %w", err)
	}
	logrus.Tracef("Ensure Azure resource group '%s' exists", cfg.GetTestResourceGroup())
	if err := cfg.EnsureGroupExists(cfg.GetTestResourceGroup(), false); err != nil {
		return fmt.Errorf("failed to ensure Azure resource group (%s) exists: %w", cfg.GetTestResourceGroup(), err)
	}
	logrus.Tracef("Ensure Azure gallery resource group '%s' exists", cfg.GetGalleryResourceGroup())
	if err := cfg.EnsureGroupExists(cfg.GetGalleryResourceGroup(), false); err != nil {
		return fmt.Errorf("failed to ensure Azure gallery resource group (%s) exists: %w", cfg.GetGalleryResourceGroup(), err)
	}
	// Ensure storage account exists
	if err := cfg.EnsureStorageAccountExists(); err != nil {
		return fmt.Errorf("failed to ensure Azure storage account exists: %w", err)
	}
	// Ensure storage container exists
	if err := cfg.EnsureStorageContainerExists(); err != nil {
		return fmt.Errorf("failed to ensure Azure storage container exists: %w", err)
	}
	// Ensure gallery exists
	if err := cfg.EnsureGalleryExists(); err != nil {
		return fmt.Errorf("failed to ensure Azure image gallery exists: %w", err)
	}

	// Ensure the image-definition exists
	if err := cfg.EnsureImageDefinitionExists(); err != nil {
		return fmt.Errorf("failed to ensure Azure image definition exists: %w", err)
	}

	return nil
}

func (cfg AzureConfig) GetWhoAmI() string {
	if cfg.WhoAmI != "" {
		return cfg.WhoAmI
	}
	whoami, err := exec.Command("whoami").Output()
	if err != nil {
		panic(fmt.Sprintf("Failed to get current user: %v", err))
	}
	return strings.TrimSpace(string(whoami))
}

func (cfg AzureConfig) GetTestResourceGroup() string {
	if cfg.TestResourceGroup != "" {
		return cfg.TestResourceGroup
	}
	return fmt.Sprintf("%s-test", cfg.GetGalleryResourceGroup())
}

func (cfg AzureConfig) GetGalleryName() string {
	if cfg.GalleryName != "" {
		return cfg.GalleryName
	}
	return fmt.Sprintf("%s_trident_gallery", cfg.GetWhoAmI())
}

func (cfg AzureConfig) GetGalleryResourceGroup() string {
	if cfg.GalleryResourceGroup != "" {
		return cfg.GalleryResourceGroup
	}
	return fmt.Sprintf("%s-trident-rg", cfg.GetWhoAmI())
}

func (cfg AzureConfig) GetStorageContainerName() string {
	if cfg.StorageContainerName != "" {
		return cfg.StorageContainerName
	}
	return fmt.Sprintf("%s-test", cfg.GetWhoAmI())
}

func (cfg AzureConfig) GetAllVmIPAddresses(vmName string, buildId string) ([]string, error) {
	ipType := "publicIps"
	if buildId != "" {
		ipType = "privateIps" // Use private IPs for build tests
	}
	logrus.Tracef("Fetching Azure VM IP addresses for type '%s'", ipType)
	cmd := exec.Command("az", "vm", "show", "-d", "-g", cfg.GetTestResourceGroup(), "-n", vmName, "--query", ipType, "-o", "tsv")
	out, err := cmd.Output()
	if err != nil {
		fullShowCmd := exec.Command("az", "vm", "show", "-d", "-g", cfg.GetTestResourceGroup(), "-n", vmName)
		fullShowCmdOut, fullShowCmdErr := fullShowCmd.CombinedOutput()
		logrus.Tracef("Failed to get Azure VM IP addresses, show vm: %v\n%s", fullShowCmdErr, fullShowCmdOut)
		return nil, fmt.Errorf("failed to get Azure VM IP (%w): %s", err, out)
	}
	return strings.Split(strings.TrimSpace(string(out)), "\n"), nil
}

func (cfg AzureConfig) GetLatestVersion() string {
	// Get existing image versions from Azure Shared Image Gallery
	out, err := exec.Command("az", "sig", "image-version", "list",
		"--resource-group", cfg.GetGalleryResourceGroup(),
		"--gallery-name", cfg.GetGalleryName(),
		"--gallery-image-definition", cfg.ImageDefinition,
		"--query", "[].name",
		"-o", "tsv").Output()
	if err != nil {
		logrus.Errorf("Failed to get latest image version: %v", err)
		return ""
	}
	versions := strings.Split(strings.TrimSpace(string(out)), "\n")
	if len(versions) == 0 {
		logrus.Info("No image versions found")
		return ""
	}
	// Sort versions by semver
	sort.Slice(versions, func(i, j int) bool {
		v1 := strings.Split(versions[i], ".")
		v2 := strings.Split(versions[j], ".")
		if len(v1) != 3 || len(v2) != 3 {
			return false // Invalid version format
		}
		for k := 0; k < 3; k++ {
			if v1[k] != v2[k] {
				return v1[k] < v2[k]
			}
		}
		return false // Versions are equal
	})
	return versions[len(versions)-1] // Return the latest version
}

func (cfg AzureConfig) GetImageVersion(buildId string, increment bool) string {
	imageVersion := "0.0.1"
	if buildId == "" {
		// If no build ID is provided, get the latest version
		imageVersion = cfg.GetLatestVersion()
		if imageVersion == "" {
			// If no versions found, use a default version
			imageVersion = "0.0.1"
		} else if increment {
			// If version was found and increment is true,
			// increment the patch version
			parts := strings.Split(imageVersion, ".")
			if len(parts) != 3 {
				logrus.Errorf("Invalid image version format: %s", imageVersion)
				return ""
			}
			major, _ := strconv.Atoi(parts[0])
			minor, _ := strconv.Atoi(parts[1])
			patch, _ := strconv.Atoi(parts[2])
			patch++ // Increment the patch version
			imageVersion = fmt.Sprintf("%d.%d.%d", major, minor, patch)
		}
	} else {
		// Use the build ID as the patch version
		imageVersion = fmt.Sprintf("0.0.%s", buildId)
	}

	logrus.Infof("Image version: %s", imageVersion)
	return imageVersion
}

func (cfg AzureConfig) SetSubscription() error {
	if _, err := cfg.CallAzCli([]string{"account", "set", "--subscription", cfg.Subscription}, false); err != nil {
		return fmt.Errorf("failed to set Azure subscription: %w", err)
	}
	return nil
}

func (cfg AzureConfig) EnsureGroupExists(groupName string, deleteExisting bool) error {
	findGroupCmdOutput, err := cfg.CallAzCli([]string{"group", "exists", "-n", groupName}, false)
	if err != nil {
		return fmt.Errorf("failed to check if group exists: %w", err)
	}

	if strings.TrimSpace(string(findGroupCmdOutput)) == "true" {
		if !deleteExisting {
			return nil
		}

		// Resource group exists, delete it
		if deleteOutput, err := cfg.CallAzCli([]string{"group", "delete", "-n", groupName, "-y"}, true); err != nil {
			return fmt.Errorf("failed to delete Azure resource group (%w):\n%s", err, deleteOutput)
		}
	}

	if groupCreateOutput, err := cfg.CallAzCli([]string{"group", "create", "-n", groupName, "-l", cfg.Region, "--tags", fmt.Sprintf("creationTime=%d", time.Now().Unix())}, true); err != nil {
		return fmt.Errorf("failed to create Azure resource group (%w):\n%s", err, groupCreateOutput)
	}
	return nil
}

func (cfg AzureConfig) DeleteGroup(groupName string) error {
	findGroupCmdOutput, err := cfg.CallAzCli([]string{"group", "exists", "-n", groupName}, false)
	if err != nil {
		return fmt.Errorf("failed to check if group exists: %w", err)
	}
	if strings.TrimSpace(string(findGroupCmdOutput)) != "true" {
		return nil
	}
	if deleteOutput, err := cfg.CallAzCli([]string{"group", "delete", "-n", groupName, "-y"}, true); err != nil {
		return fmt.Errorf("failed to delete Azure resource group (%w):\n%s", err, deleteOutput)
	}
	return nil
}

func (cfg AzureConfig) EnsureStorageAccountExists() error {
	// Ensure storage account exists
	storageAccountResourceId := fmt.Sprintf("/subscriptions/%s/resourceGroups/%s/providers/Microsoft.Storage/storageAccounts/%s",
		cfg.Subscription, cfg.StorageAccountResourceGroup, cfg.StorageAccount)

	logrus.Tracef("Ensure Azure storage account '%s' exists in resource group '%s'", cfg.StorageAccount, cfg.StorageAccountResourceGroup)
	storageAccountOutput, err := cfg.CallAzCli(
		[]string{
			"storage", "account", "show",
			"--ids", storageAccountResourceId,
		},
		false,
	)
	if err != nil || strings.TrimSpace(string(storageAccountOutput)) == "" {
		logrus.Infof("Could not find storage account '%s' in the expected location. Creating the storage account.", cfg.StorageAccount)
		checkNameOut, err := cfg.CallAzCli(
			[]string{"storage", "account", "check-name", "--name", cfg.StorageAccount, "--query", "nameAvailable"},
			false,
		)
		if err != nil || strings.TrimSpace(string(checkNameOut)) != "false" {
			return fmt.Errorf("storage account name %s is not available", cfg.StorageAccount)
		}
		createStorageOutput, err := cfg.CallAzCli(
			[]string{
				"storage", "account", "create",
				"-g", cfg.StorageAccountResourceGroup,
				"-n", cfg.StorageAccount,
				"-l", cfg.Region,
				"--allow-shared-key-access", "false",
			},
			true,
		)
		if err != nil {
			logrus.Tracef("Failed to create Azure storage account '%s': %s", cfg.StorageAccount, createStorageOutput)
			return fmt.Errorf("failed to create Azure storage account: %w", err)
		}
	}
	return nil
}

func (cfg AzureConfig) EnsureStorageContainerExists() error {
	logrus.Tracef("Ensure Azure storage container '%s' exists in storage account '%s'", cfg.GetStorageContainerName(), cfg.StorageAccount)
	containerExistsOutput, err := cfg.CallAzCli(
		[]string{
			"storage", "container", "exists",
			"--account-name", cfg.StorageAccount,
			"--name", cfg.GetStorageContainerName(),
			"--auth-mode", "login",
		},
		false,
	)
	if err != nil || !strings.Contains(string(containerExistsOutput), `"exists": true`) {
		logrus.Tracef("Container '%s' not found, creating in storage account '%s'...", cfg.GetStorageContainerName(), cfg.StorageAccount)
		createContainerOutput, err := cfg.CallAzCli(
			[]string{
				"storage", "container", "create",
				"--account-name", cfg.StorageAccount,
				"--name", cfg.GetStorageContainerName(),
				"--auth-mode", "login",
			},
			true,
		)
		if err != nil {
			logrus.Tracef("Failed to create Azure storage container '%s': %s", cfg.GetStorageContainerName(), createContainerOutput)
			return fmt.Errorf("failed to create Azure storage container: %w", err)
		}
	}
	return nil
}

func (cfg AzureConfig) EnsureGalleryExists() error {
	logrus.Tracef("Ensure Azure image gallery '%s' exists in resource group '%s'", cfg.GetGalleryName(), cfg.GetGalleryResourceGroup())
	galleryExistsOutput, err := cfg.CallAzCli(
		[]string{
			"sig", "show",
			"-r", cfg.GetGalleryName(),
			"-g", cfg.GetGalleryResourceGroup(),
		},
		false,
	)
	if err != nil || strings.TrimSpace(string(galleryExistsOutput)) == "" {
		logrus.Infof("Could not find image gallery '%s' in resource group '%s'. Creating the gallery.", cfg.GetGalleryName(), cfg.GetGalleryResourceGroup())
		createGalleryOutput, err := cfg.CallAzCli(
			[]string{
				"sig", "create",
				"-g", cfg.GetGalleryResourceGroup(),
				"-r", cfg.GetGalleryName(),
				"-l", cfg.Region,
			},
			true)
		if err != nil {
			logrus.Tracef("Failed to create Azure image gallery '%s': %s", cfg.GetGalleryName(), createGalleryOutput)
			return fmt.Errorf("failed to create Azure image gallery: %w", err)
		}
	}
	return nil
}

func (cfg AzureConfig) EnsureImageDefinitionExists() error {
	logrus.Tracef("Ensure Azure image definition '%s' exists in gallery '%s'", cfg.ImageDefinition, cfg.GetGalleryName())
	imageDefinitionExistsOutput, err := cfg.CallAzCli(
		[]string{
			"sig", "image-definition", "list",
			"-r", cfg.GetGalleryName(),
			"-g", cfg.GetGalleryResourceGroup(),
			"--query", fmt.Sprintf("[?name=='%s'].name", cfg.ImageDefinition),
			"-o", "tsv",
		},
		false,
	)
	if err != nil || strings.TrimSpace(string(imageDefinitionExistsOutput)) == "" {
		logrus.Infof("Could not find image-definition '%s'. Creating definition '%s' in gallery '%s'...", cfg.ImageDefinition, cfg.ImageDefinition, cfg.GetGalleryName())
		createImageDefOutput, err := cfg.CallAzCli(
			[]string{
				"sig", "image-definition", "create",
				"-i", cfg.ImageDefinition,
				"--publisher", cfg.GetWhoAmI(),
				"--offer", cfg.Offer,
				"--sku", cfg.ImageDefinition,
				"-r", cfg.GetGalleryName(),
				"-g", cfg.GetGalleryResourceGroup(),
				"--os-type", "Linux",
			},
			true,
		)
		if err != nil {
			logrus.Tracef("Failed to create Azure image definition '%s': %s", cfg.ImageDefinition, createImageDefOutput)
			return fmt.Errorf("failed to create Azure image definition: %w", err)
		}
	}
	return nil
}

func (cfg AzureConfig) CallAzCli(azArgs []string, combined bool) (string, error) {
	cmd := exec.Command("az", azArgs...)
	var b bytes.Buffer
	cmd.Stdout = &b
	if combined {
		cmd.Stderr = &b
	}
	err := cmd.Run()
	return b.String(), err
}
