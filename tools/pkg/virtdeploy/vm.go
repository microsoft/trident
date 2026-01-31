package virtdeploy

import (
	"fmt"
	"tridenttools/pkg/ref"

	"libvirt.org/go/libvirtxml"
)

// Helper to check VM architecture
func (vm *VirtDeployVM) isArm64() bool {
	return vm.Arch == "arm64"
}

// asXml renders the libvirt domain XML corresponding to the VM definition.
// It translates the earlier XML template into structured Go objects.
// Some low-level address/controller elements are omitted for brevity; libvirt
// will auto-assign them. Extend if deterministic addressing is required.
func (vm *VirtDeployVM) asXml(network *virtDeployNetwork, nvramPool storagePool) (string, error) {
	if vm.isArm64() {
		return vm.asArm64Xml(network, nvramPool)
	}
	return vm.asAmd64Xml(network, nvramPool)
}

func (vm *VirtDeployVM) configureDisks() []libvirtxml.DomainDisk {
	disks := make([]libvirtxml.DomainDisk, 0, len(vm.volumes)+len(vm.cdroms))
	for i, vol := range vm.volumes {
		domainDisk := libvirtxml.DomainDisk{
			Device: "disk",
			Driver: &libvirtxml.DomainDiskDriver{
				Name: "qemu",
				Type: "qcow2",
			},
			Source: &libvirtxml.DomainDiskSource{
				File: &libvirtxml.DomainDiskSourceFile{File: vol.path},
			},
			Target: &libvirtxml.DomainDiskTarget{
				Dev: vol.device, // e.g. sda, sdb
			},
			Address: &libvirtxml.DomainAddress{},
		}
		if vm.isArm64() {
			domainDisk.Target.Bus = "virtio"
			domainDisk.Address.PCI = &libvirtxml.DomainAddressPCI{
				Domain:   new(uint),
				Bus:      new(uint),
				Slot:     new(uint),
				Function: new(uint),
			}
		} else {
			domainDisk.Target.Bus = "sata"
			domainDisk.Address.Drive = &libvirtxml.DomainAddressDrive{
				Controller: new(uint),
				Bus:        new(uint),
				Target:     new(uint),
				Unit:       ref.Of(uint(i + 1)),
			}
		}
		disks = append(disks, domainDisk)
	}
	for i, cd := range vm.cdroms {
		addressDrive := &libvirtxml.DomainAddressDrive{
			Controller: ref.Of(uint(1)),
			Bus:        new(uint),
			Target:     new(uint),
			Unit:       ref.Of(uint(i + 1)),
		}
		d := libvirtxml.DomainDisk{
			Device:   "cdrom",
			Driver:   &libvirtxml.DomainDiskDriver{Name: "qemu", Type: "raw"},
			Target:   &libvirtxml.DomainDiskTarget{Dev: cd.device, Bus: "sata"},
			ReadOnly: &libvirtxml.DomainDiskReadOnly{},
			Address: &libvirtxml.DomainAddress{
				Drive: addressDrive,
			},
		}
		if cd.path != "" {
			d.Source = &libvirtxml.DomainDiskSource{File: &libvirtxml.DomainDiskSourceFile{File: cd.path}}
		}
		disks = append(disks, d)
	}
	return disks
}

func (vm *VirtDeployVM) configureTpms() []libvirtxml.DomainTPM {
	// Optional TPM
	var tpms []libvirtxml.DomainTPM
	if vm.EmulatedTPM {
		tpms = []libvirtxml.DomainTPM{{
			Model: "tpm-tis",
			Backend: &libvirtxml.DomainTPMBackend{
				Emulator: &libvirtxml.DomainTPMBackendEmulator{
					Version: "2.0",
				},
			},
		}}
	}
	return tpms
}

func (vm *VirtDeployVM) createDomain(
	network *virtDeployNetwork, nvramPool storagePool,
	domainType string,
	osType libvirtxml.DomainOSType,
	cpuModel libvirtxml.DomainCPUModel,
	cpuFeatures []libvirtxml.DomainCPUFeature,
	pm *libvirtxml.DomainPM,
	timer []libvirtxml.DomainTimer,
	emulator string,
	apic *libvirtxml.DomainFeatureAPIC,
	vmPort *libvirtxml.DomainFeatureState,
	controllers []libvirtxml.DomainController,
) libvirtxml.Domain {
	return libvirtxml.Domain{
		Type:   domainType,
		Name:   vm.name,
		Memory: &libvirtxml.DomainMemory{Unit: "GiB", Value: vm.Mem},
		VCPU:   &libvirtxml.DomainVCPU{Value: vm.Cpus},
		OS: &libvirtxml.DomainOS{

			Type:   &osType,
			SMBios: &libvirtxml.DomainSMBios{Mode: "sysinfo"},
			Loader: &libvirtxml.DomainLoader{Path: vm.firmwareLoaderPath, Type: "pflash", Readonly: "yes"},
			NVRam: &libvirtxml.DomainNVRam{
				NVRam:    vm.nvramPath,
				Template: vm.firmwareVarsTemplatePath,
			},
		},
		SysInfo: []libvirtxml.DomainSysInfo{
			{
				SMBIOS: &libvirtxml.DomainSysInfoSMBIOS{
					OEMStrings: &libvirtxml.DomainSysInfoOEMStrings{
						Entry: []string{"virtdeploy:1"},
					},
				},
			},
		},
		Features: &libvirtxml.DomainFeatureList{
			ACPI:   &libvirtxml.DomainFeature{},
			APIC:   apic,
			VMPort: vmPort,
		},
		CPU: &libvirtxml.DomainCPU{
			Match:    "exact",
			Check:    "none",
			Model:    &cpuModel,
			Features: cpuFeatures,
		},
		Clock: &libvirtxml.DomainClock{
			Offset: "utc",
			Timer:  timer,
		},
		PM: pm,
		Devices: &libvirtxml.DomainDeviceList{
			Emulator:    emulator,
			Disks:       vm.configureDisks(),
			Controllers: controllers,
			Interfaces: []libvirtxml.DomainInterface{{
				Model: &libvirtxml.DomainInterfaceModel{Type: "virtio"},
				MAC:   &libvirtxml.DomainInterfaceMAC{Address: vm.mac.String()},
				Source: &libvirtxml.DomainInterfaceSource{
					Network: &libvirtxml.DomainInterfaceSourceNetwork{
						Network: network.name,
					},
				},
			}},
			Consoles: []libvirtxml.DomainConsole{{
				Source: &libvirtxml.DomainChardevSource{
					Pty: &libvirtxml.DomainChardevSourcePty{},
				},
			}},
			Channels: []libvirtxml.DomainChannel{{
				Source: &libvirtxml.DomainChardevSource{
					SpiceVMC: &libvirtxml.DomainChardevSourceSpiceVMC{},
				},
				Target: &libvirtxml.DomainChannelTarget{
					VirtIO: &libvirtxml.DomainChannelTargetVirtIO{
						Name: "com.redhat.spice.0",
					},
				},
			}},
			Inputs: []libvirtxml.DomainInput{{Type: "tablet", Bus: "usb"}},
			RedirDevs: []libvirtxml.DomainRedirDev{
				{Bus: "usb", Source: &libvirtxml.DomainChardevSource{
					SpiceVMC: &libvirtxml.DomainChardevSourceSpiceVMC{},
				}},
				{Bus: "usb", Source: &libvirtxml.DomainChardevSource{
					SpiceVMC: &libvirtxml.DomainChardevSourceSpiceVMC{},
				}},
			},
			Serials: []libvirtxml.DomainSerial{{
				Source: &libvirtxml.DomainChardevSource{
					Pty: &libvirtxml.DomainChardevSourcePty{
						Path: "/dev/pts/2",
					},
				},
				Target: &libvirtxml.DomainSerialTarget{Port: new(uint)},
				Log:    &libvirtxml.DomainChardevLog{File: fmt.Sprintf("/tmp/%s-serial0.log", vm.name), Append: "off"},
			}},
			TPMs: vm.configureTpms(),
		},
	}
}

func (vm *VirtDeployVM) asAmd64Xml(network *virtDeployNetwork, nvramPool storagePool) (string, error) {
	// AMD64-specific domain XML
	dom := vm.createDomain(
		network,
		nvramPool,
		"kvm",
		libvirtxml.DomainOSType{Arch: "x86_64", Machine: "q35", Type: "hvm"},
		libvirtxml.DomainCPUModel{Fallback: "allow", Value: "Broadwell-IBRS"},
		[]libvirtxml.DomainCPUFeature{{Policy: "require", Name: "vmx"}},
		&libvirtxml.DomainPM{
			SuspendToMem:  &libvirtxml.DomainPMPolicy{Enabled: "no"},
			SuspendToDisk: &libvirtxml.DomainPMPolicy{Enabled: "no"},
		},
		[]libvirtxml.DomainTimer{
			{Name: "rtc", TickPolicy: "catchup"},
			{Name: "pit", TickPolicy: "delay"},
			{Name: "hpet", Present: "no"},
		},
		"/usr/bin/qemu-system-x86_64",
		&libvirtxml.DomainFeatureAPIC{},
		&libvirtxml.DomainFeatureState{State: "off"},
		[]libvirtxml.DomainController{
			{Type: "usb", Index: new(uint), Model: "ich9-ehci1"},
			{Type: "usb", Index: new(uint), Model: "ich9-uhci1", USB: &libvirtxml.DomainControllerUSB{
				Master: &libvirtxml.DomainControllerUSBMaster{
					StartPort: 0,
				},
			}},
			{Type: "usb", Index: new(uint), Model: "ich9-uhci2", USB: &libvirtxml.DomainControllerUSB{
				Master: &libvirtxml.DomainControllerUSBMaster{
					StartPort: 2,
				},
			}},
			{Type: "usb", Index: new(uint), Model: "ich9-uhci3", USB: &libvirtxml.DomainControllerUSB{
				Master: &libvirtxml.DomainControllerUSBMaster{
					StartPort: 4,
				},
			}},
		},
	)

	xml, err := dom.Marshal()
	if err != nil {
		return "", fmt.Errorf("marshal amd64 domain to XML: %w", err)
	}
	return xml, nil
}

func (vm *VirtDeployVM) asArm64Xml(network *virtDeployNetwork, nvramPool storagePool) (string, error) {
	// ARM64-specific domain XML
	dom := vm.createDomain(
		network,
		nvramPool,
		"qemu",
		libvirtxml.DomainOSType{Arch: "aarch64", Machine: "virt-6.2", Type: "hvm"},
		libvirtxml.DomainCPUModel{Fallback: "forbid", Value: "cortex-a57"},
		[]libvirtxml.DomainCPUFeature{},
		nil,
		[]libvirtxml.DomainTimer{},
		"/usr/bin/qemu-system-aarch64",
		nil,
		nil,
		[]libvirtxml.DomainController{
			{Type: "usb", Index: new(uint), Model: "qemu-xhci", Alias: &libvirtxml.DomainAlias{Name: "usb"}},
		},
	)

	xml, err := dom.Marshal()
	if err != nil {
		return "", fmt.Errorf("marshal arm64 domain to XML: %w", err)
	}
	return xml, nil
}
