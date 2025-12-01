package scenario

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"tridenttools/storm/utils/cmd"
	"tridenttools/storm/utils/env"

	"github.com/microsoft/storm"
	log "github.com/sirupsen/logrus"
)

func (s *TridentE2EScenario) installVmDependencies(tc storm.TestCase) error {
	if !s.args.PipelineRun {
		tc.Skip("local run")
	}

	if s.hardware != HardwareTypeVM {
		tc.Skip("not a VM scenario")
	}

	log.Info("Installing VM dependencies...")

	osRelease, err := env.ParseOsRelease()
	if err != nil {
		return fmt.Errorf("failed to parse os-release: %w", err)
	}

	err = nil
	switch osRelease.Id {
	case "ubuntu":
		err = installUbuntuDependencies(osRelease)
	default:
		return fmt.Errorf("unsupported OS for dependency installation: %s", osRelease.Id)
	}

	if err != nil {
		return fmt.Errorf("failed to install dependencies: %w", err)
	}

	err = configureLibvirtAccess()
	if err != nil {
		return fmt.Errorf("failed to configure libvirt access: %w", err)
	}

	log.Info("Dependencies installed successfully")

	return nil
}

func installUbuntuDependencies(osRelease *env.OsReleaseInfo) error {
	if osRelease.Id != "ubuntu" {
		return fmt.Errorf("unsupported OS for dependency installation on ubuntu: %s", osRelease.Id)
	}

	err := prepareSwtpmUbuntu(osRelease)
	if err != nil {
		return fmt.Errorf("failed to install swtpm: %w", err)
	}

	err = cmd.Run("sudo", "NEEDRESTART_MODE=a",
		"apt-get", "-y",
		"swtpm",
		"swtpm-tools",
		"bridge-utils",
		"virt-manager",
		"qemu-efi=2022.02-3ubuntu0.22.04.3",
		"qemu-kvm",
		"libtpms0",
		"libvirt-daemon-system",
		"libvirt-clients",
		"python3-libvirt",
		"ovmf=2022.02-3ubuntu0.22.04.3",
		"openssl",
		"python3-netifaces",
		"python3-docker",
		"python3-bcrypt",
		"python3-jinja2",
		"zstd",
		"imagemagick",
	)

	if err != nil {
		return fmt.Errorf("failed to install ubuntu dependencies: %w", err)
	}

	return nil
}

func prepareSwtpmUbuntu(osRelease *env.OsReleaseInfo) error {
	swtpmTargetUbuntuCodenames := []string{"focal", "jammy"}
	if !slices.Contains(swtpmTargetUbuntuCodenames, osRelease.VersionCodename) {
		// Skip swtpm installation on unsupported Ubuntu versions
		log.Debugf("swtpm installation skipped on Ubuntu '%s'", osRelease.VersionCodename)
		return nil
	}

	var err error
	err = cmd.Run("sudo", "add-apt-repository", "-y", "ppa:stefanberger/swtpm-"+osRelease.VersionCodename)
	if err != nil {
		return fmt.Errorf("failed to add swtpm ppa: %w", err)
	}
	err = cmd.Run("sudo", "apt-get", "-y", "update")
	if err != nil {
		return fmt.Errorf("failed to update apt-get: %w", err)
	}

	// This prevents conflict on later install.
	// It fails silently if the packages aren't installed.
	err = cmd.Run("sudo", "apt-get", "purge", "-y", "swtpm", "swtpm-tools", "libtpms0")
	if err != nil {
		return fmt.Errorf("failed to purge swtpm packages: %w", err)
	}

	return nil
}

func configureLibvirtAccess() error {
	err := cmd.Run("sudo", "usermod", "-aG", "libvirt", "$USER")
	if err != nil {
		return fmt.Errorf("failed to add user to libvirt group: %w", err)
	}

	//Get user's home directory
	homeDir, err := os.UserHomeDir()
	if err != nil {
		return fmt.Errorf("failed to get user's home directory: %w", err)
	}

	//Create ~/.config/libvirt directory if it doesn't exist
	libVirtConfigDir := filepath.Join(homeDir, ".config", "libvirt")
	err = os.MkdirAll(libVirtConfigDir, 0755)
	if err != nil {
		return fmt.Errorf("failed to create libvirt config directory: %w", err)
	}

	//Create empty libvirt.conf file if it doesn't exist
	libVirtConfFile := filepath.Join(libVirtConfigDir, "libvirt.conf")
	err = os.WriteFile(libVirtConfFile, []byte("uri_default = \"qemu:///system\""), 0644)
	if err != nil {
		return fmt.Errorf("failed to create libvirt.conf file: %w", err)
	}

	err = cmd.RunGroup(
		cmd.Cmd("sudo", "mkdir", "-p", "/etc/systemd/system/libvirtd.socket.d"),
		cmd.Cmd("sudo", "bash", "-c", "echo '[Socket]\\nListenStream=/var/run/libvirt/libvirt-sock' > /etc/systemd/system/libvirtd.socket.d/override.conf"),
		cmd.Cmd("sudo", "systemctl", "daemon-reload"),
		cmd.Cmd("sudo", "systemctl", "restart", "libvirtd.socket"),
	)
	if err != nil {
		return fmt.Errorf("failed to configure libvirt socket: %w", err)
	}

	return nil
}
