package virtdeploy

import (
	"fmt"
	"path"
	"tridenttools/pkg/ref"

	"libvirt.org/go/libvirtxml"
)

const (
	FIRMWARE_LOADER_PATH            = "/usr/share/OVMF/OVMF_CODE_4M.fd"
	FIRMWARE_LOADER_PATH_SECUREBOOT = "/usr/share/OVMF/OVMF_CODE_4M.ms.fd"
)

// asXml renders the libvirt domain XML corresponding to the VM definition.
// It translates the earlier XML template into structured Go objects.
// Some low-level address/controller elements are omitted for brevity; libvirt
// will auto-assign them. Extend if deterministic addressing is required.
func (vm *VirtDeployVM) asXml(network *virtDeployNetwork, nvramPool storagePool) (string, error) {
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
					Unit:       ref.Of(uint(i)),
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
					Unit:       ref.Of(uint(i)),
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

	// Firmware / UEFI
	loaderPath := FIRMWARE_LOADER_PATH
	if vm.SecureBoot {
		loaderPath = FIRMWARE_LOADER_PATH_SECUREBOOT
	}

	// NVRAM path per template
	nvramPath := path.Join(nvramPool.path, fmt.Sprintf("%s_VARS.fd", vm.name))

	dom := libvirtxml.Domain{
		Type:   "kvm",
		Name:   vm.name,
		Memory: &libvirtxml.DomainMemory{Unit: "GiB", Value: vm.Mem},
		VCPU:   &libvirtxml.DomainVCPU{Value: vm.Cpus},
		OS: &libvirtxml.DomainOS{

			Type:   &libvirtxml.DomainOSType{Arch: "x86_64", Machine: "q35", Type: "hvm"},
			SMBios: &libvirtxml.DomainSMBios{Mode: "sysinfo"},
			Loader: &libvirtxml.DomainLoader{Path: loaderPath, Type: "pflash", Readonly: "yes"},
			NVRam:  &libvirtxml.DomainNVRam{NVRam: nvramPath},
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
			VMPort: &libvirtxml.DomainFeatureState{State: "off"},
		},
		CPU: &libvirtxml.DomainCPU{
			Match:    "exact",
			Check:    "none",
			Model:    &libvirtxml.DomainCPUModel{Fallback: "allow", Value: "Broadwell-IBRS"},
			Features: []libvirtxml.DomainCPUFeature{{Policy: "require", Name: "vmx"}},
		},
		Clock: &libvirtxml.DomainClock{
			Offset: "utc",
			Timer: []libvirtxml.DomainTimer{
				{Name: "rtc", TickPolicy: "catchup"},
				{Name: "pit", TickPolicy: "delay"},
				{Name: "hpet", Present: "no"},
			},
		},
		PM: &libvirtxml.DomainPM{
			SuspendToMem:  &libvirtxml.DomainPMPolicy{Enabled: "no"},
			SuspendToDisk: &libvirtxml.DomainPMPolicy{Enabled: "no"},
		},
		Devices: &libvirtxml.DomainDeviceList{
			Emulator: "/usr/bin/qemu-system-x86_64",
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
			Sounds: []libvirtxml.DomainSound{{
				Model: "ich6",
			}},
			Videos: []libvirtxml.DomainVideo{{
				Model: libvirtxml.DomainVideoModel{Type: "qxl"},
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
