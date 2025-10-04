package main

import (
	"encoding/json"
	"fmt"
	"net"
	"os"
	"tridenttools/pkg/config"
	"tridenttools/pkg/ref"
	"tridenttools/pkg/virtdeploy"

	"github.com/alecthomas/kong"
	log "github.com/sirupsen/logrus"
	"gopkg.in/yaml.v3"
)

const (
	DEFAULT_NAMESPACE = "virtdeploy"
	DEFAULT_NETWORK   = "192.168.242.0/24"
)

var cli struct {
	Verbosity log.Level    `short:"v" long:"verbosity" description:"Increase verbosity" default:"info"`
	CreateOne CreateOneCmd `cmd:"create" description:"Create one VM"`
	Clean     CleanCmd     `cmd:"clean" description:"Delete all resources in a namespace"`
}

func main() {
	ctx := kong.Parse(
		&cli,
		kong.Name("virtdeploy"),
		kong.Description("Tool to deploy VMs using libvirt and KVM"),
		kong.UsageOnError(),
		kong.Vars{
			"DEFAULT_NAMESPACE": DEFAULT_NAMESPACE,
			"DEFAULT_NETWORK":   DEFAULT_NETWORK,
		},
	)
	log.SetLevel(cli.Verbosity)
	err := ctx.Run()
	ctx.FatalIfErrorf(err)
}

type CleanCmd struct {
	Namespace string `short:"n" long:"namespace" help:"Namespace to clean up" default:"${DEFAULT_NAMESPACE}"`
}

func (c *CleanCmd) Run() error {
	err := virtdeploy.DeleteResources(c.Namespace)
	if err != nil {
		return fmt.Errorf("clean failed: %w", err)
	}

	log.Infof("Cleaned up namespace: %s", c.Namespace)
	return nil
}

type CreateOneCmd struct {
	Namespace    string `group:"Resource" short:"n" long:"namespace" help:"Namespace to create resources in" default:"${DEFAULT_NAMESPACE}"`
	Network      string `group:"Resource" short:"N" long:"network" help:"Network to create resources in" default:"${DEFAULT_NETWORK}"`
	CPUs         uint   `group:"VM" short:"c" long:"cpus" help:"Number of CPUs for the VM" default:"4"`
	Mem          uint   `group:"VM" short:"m" long:"mem" help:"Memory in GB for the VM" default:"6"`
	Disks        []uint `group:"VM" short:"d" long:"disk" help:"Disk sizes in GB for the VM" default:"32"`
	NoSecureBoot bool   `group:"VM" long:"no-secure-boot" help:"Disable secure boot"`
	NoTpm        bool   `group:"VM" long:"no-tpm" help:"Disable emulated TPM"`
	OsDisk       string `group:"VM" long:"os-disk" help:"Optional path to an OS disk image to attach to the first disk"`
	CiUser       string `group:"cloud-init" and:"ci-meta" long:"ci-user" help:"Cloud-init userdata file path" type:"existingfile"`
	CiMeta       string `group:"cloud-init" and:"ci-user" long:"ci-meta" help:"Cloud-init metadata file path" type:"existingfile"`
	Json         bool   `group:"Output" short:"J" long:"json" help:"Output resource data as JSON to stdout."`
	Netlaunch    string `group:"Output" short:"l" long:"netlaunch" type:"path" help:"Produce a netlaunch YAML at the given path" default:"./tools/vm-netlaunch.yaml"`
}

func (c *CreateOneCmd) Run() error {
	_, network, err := net.ParseCIDR(c.Network)
	if err != nil {
		return fmt.Errorf("invalid network: %w", err)
	}

	if network == nil {
		return fmt.Errorf("invalid network")
	}

	var cloudInitConfig *virtdeploy.CloudInitConfig
	if c.CiMeta != "" && c.CiUser == "" {
		return fmt.Errorf("a cloud-init user file must be specified if a metadata file is provided")
	} else if c.CiUser != "" && c.CiMeta == "" {
		return fmt.Errorf("a cloud-init metadata file must be specified if a user file is provided")
	} else if c.CiUser != "" && c.CiMeta != "" {
		log.Infof("Using cloud-init user file: %s", c.CiUser)
		log.Infof("Using cloud-init metadata file: %s", c.CiMeta)
		cloudInitConfig = &virtdeploy.CloudInitConfig{
			Userdata: c.CiUser,
			Metadata: c.CiMeta,
		}
	}

	status, err := virtdeploy.CreateResources(virtdeploy.VirtDeployConfig{
		Namespace:    c.Namespace,
		IPNet:        *network,
		NatInterface: virtdeploy.AutoDetectNatInterface,
		VMs: []virtdeploy.VirtDeployVM{
			{
				Cpus:        c.CPUs,
				Mem:         c.Mem,
				Disks:       c.Disks,
				SecureBoot:  !c.NoSecureBoot,
				EmulatedTPM: !c.NoTpm,
				OsDiskPath:  c.OsDisk,
				CloudInit:   cloudInitConfig,
			},
		},
	})

	if err != nil {
		return fmt.Errorf("create-one failed: %w", err)
	}

	log.Info("Finished creating resources")

	if c.Json {
		marshalled, err := json.MarshalIndent(status, "", "    ")
		if err != nil {
			return fmt.Errorf("failed to marshal JSON: %w", err)
		}
		fmt.Println(string(marshalled))
	}

	if c.Netlaunch != "" {
		// Netlaunch can figure out everything but the VM UUID itself.
		config := config.NetLaunchConfig{
			Netlaunch: config.NetlaunchConfigInner{
				LocalVmUuid:  ref.Of(status.VMs[0].Uuid.String()),
				LocalVmNvRam: &status.VMs[0].NvramPath,
			},
		}

		yamlData, err := yaml.Marshal(config)
		if err != nil {
			return fmt.Errorf("failed to marshal YAML: %w", err)
		}

		err = os.WriteFile(c.Netlaunch, yamlData, 0644)
		if err != nil {
			return fmt.Errorf("failed to write netlaunch file: %w", err)
		}
	}

	return nil
}
