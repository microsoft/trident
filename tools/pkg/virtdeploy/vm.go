package virtdeploy

import (
	"fmt"
	"runtime"
	"tridenttools/pkg/ref"

	"libvirt.org/go/libvirtxml"
)

// asXml renders the libvirt domain XML corresponding to the VM definition.
// It translates the earlier XML template into structured Go objects.
// Some low-level address/controller elements are omitted for brevity; libvirt
// will auto-assign them. Extend if deterministic addressing is required.
func (vm *VirtDeployVM) asXml(network *virtDeployNetwork, nvramPool storagePool) (string, error) {
	// Check machine architecture
	domainType := "kvm"
	osType := libvirtxml.DomainOSType{Arch: "x86_64", Machine: "q35", Type: "hvm"}
	cpuModel := libvirtxml.DomainCPUModel{Fallback: "allow", Value: "Broadwell-IBRS"}
	cpuFeatures := []libvirtxml.DomainCPUFeature{{Policy: "require", Name: "vmx"}}
	pm := &libvirtxml.DomainPM{
		SuspendToMem:  &libvirtxml.DomainPMPolicy{Enabled: "no"},
		SuspendToDisk: &libvirtxml.DomainPMPolicy{Enabled: "no"},
	}
	emulator := "/usr/bin/qemu-system-x86_64"
	vmPort := &libvirtxml.DomainFeatureState{State: "off"}
	if runtime.GOARCH == "arm64" {
		domainType = "qemu"
		osType = libvirtxml.DomainOSType{Arch: "aarch64", Machine: "virt-6.2", Type: "hvm"}
		cpuModel = libvirtxml.DomainCPUModel{Fallback: "forbid", Value: "cortex-a57"}
		cpuFeatures = []libvirtxml.DomainCPUFeature{}
		pm = nil
		emulator = "/usr/bin/qemu-system-aarch64"
		vmPort = nil
	}

	// Build disks (regular volumes)
	disks := make([]libvirtxml.DomainDisk, 0, len(vm.volumes)+len(vm.cdroms))
	for i, vol := range vm.volumes {
		disks = append(disks, libvirtxml.DomainDisk{
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
				Bus: "sata",
			},
			Address: &libvirtxml.DomainAddress{
				Drive: &libvirtxml.DomainAddressDrive{
					Controller: new(uint),
					Bus:        new(uint),
					Target:     new(uint),
					Unit:       ref.Of(uint(i + 1)),
				},
			},
		})
	}

	// Build CDROM devices
	for i, cd := range vm.cdroms {
		d := libvirtxml.DomainDisk{
			Device:   "cdrom",
			Driver:   &libvirtxml.DomainDiskDriver{Name: "qemu", Type: "raw"},
			Target:   &libvirtxml.DomainDiskTarget{Dev: cd.device, Bus: "sata"},
			ReadOnly: &libvirtxml.DomainDiskReadOnly{},
			Address: &libvirtxml.DomainAddress{
				Drive: &libvirtxml.DomainAddressDrive{
					Controller: ref.Of(uint(1)),
					Bus:        new(uint),
					Target:     new(uint),
					Unit:       ref.Of(uint(i + 1)),
				},
			},
		}
		if cd.path != "" {
			d.Source = &libvirtxml.DomainDiskSource{File: &libvirtxml.DomainDiskSourceFile{File: cd.path}}
		}
		disks = append(disks, d)
	}

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

	// Network interface (single)
	ifaces := []libvirtxml.DomainInterface{{
		Model: &libvirtxml.DomainInterfaceModel{Type: "virtio"},
		MAC:   &libvirtxml.DomainInterfaceMAC{Address: vm.mac.String()},
		Source: &libvirtxml.DomainInterfaceSource{
			Network: &libvirtxml.DomainInterfaceSourceNetwork{
				Network: network.name,
			},
		},
	}}

	dom := libvirtxml.Domain{
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
			APIC:   &libvirtxml.DomainFeatureAPIC{},
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
			Timer: []libvirtxml.DomainTimer{
				{Name: "rtc", TickPolicy: "catchup"},
				{Name: "pit", TickPolicy: "delay"},
				{Name: "hpet", Present: "no"},
			},
		},
		PM: pm,
		Devices: &libvirtxml.DomainDeviceList{
			Emulator: emulator,
			Disks:    disks,
			Controllers: []libvirtxml.DomainController{
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
			Interfaces: ifaces,
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
			Graphics: []libvirtxml.DomainGraphic{{
				Spice: &libvirtxml.DomainGraphicSpice{
					Port:     -1,
					TLSPort:  -1,
					AutoPort: "yes",
					Image: &libvirtxml.DomainGraphicSpiceImage{
						Compression: "off",
					},
				},
			}},
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
			TPMs: tpms,
		},
	}

	xml, err := dom.Marshal()
	if err != nil {
		return "", fmt.Errorf("marshal domain to XML: %w", err)
	}
	return xml, nil
}
