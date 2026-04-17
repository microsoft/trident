package virtdeploy

import (
	"fmt"
	"tridenttools/pkg/ref"

	"libvirt.org/go/libvirtxml"
)

// asXml renders the libvirt domain XML corresponding to the VM definition.
// It translates the earlier XML template into structured Go objects.
// Some low-level address/controller elements are omitted for brevity; libvirt
// will auto-assign them. Extend if deterministic addressing is required.
func (vm *VirtDeployVM) asXml() (string, error) {
	dom := vm.createDomain()
	xml, err := dom.Marshal()
	if err != nil {
		return "", fmt.Errorf("marshal domain to XML: %w", err)
	}
	return xml, nil
}

// createDomain assembles the full libvirt Domain struct from the VM's
// configuration. Each arch-dependent field is populated by a dedicated
// configure method.
func (vm *VirtDeployVM) createDomain() libvirtxml.Domain {
	var networkInterfaces []libvirtxml.DomainInterface
	if vm.networkName != "" {
		networkInterfaces = []libvirtxml.DomainInterface{{
			Model: &libvirtxml.DomainInterfaceModel{Type: "virtio"},
			MAC:   &libvirtxml.DomainInterfaceMAC{Address: vm.mac.String()},
			Source: &libvirtxml.DomainInterfaceSource{
				Network: &libvirtxml.DomainInterfaceSourceNetwork{
					Network: vm.networkName,
				},
			},
		}}
	}

	var secLabels []libvirtxml.DomainSecLabel
	var qemuExtraArgs []libvirtxml.DomainQEMUCommandlineArg
	if vm.ignitionVolume != "" {
		qemuExtraArgs = []libvirtxml.DomainQEMUCommandlineArg{
			{Value: "-fw_cfg"},
			{Value: fmt.Sprintf("name=opt/org.flatcar-linux/config,file=%s", vm.ignitionVolume)},
		}

		// If we're passing an Ignition config, we need to disable apparmor
		// isolation to allow QEMU to read the config file from the host
		// filesystem.
		secLabels = []libvirtxml.DomainSecLabel{
			{Type: "none"},
		}
	}

	osType := vm.configureOSType()
	cpuModel := vm.configureCPUModel()

	return libvirtxml.Domain{
		Type:   vm.configureDomainType(),
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
			APIC:   vm.configureAPIC(),
			VMPort: vm.configureVMPort(),
		},
		CPU: &libvirtxml.DomainCPU{
			Match:    "exact",
			Check:    "none",
			Model:    &cpuModel,
			Features: vm.configureCPUFeatures(),
		},
		Clock: &libvirtxml.DomainClock{
			Offset: "utc",
			Timer:  vm.configureTimers(),
		},
		PM: vm.configurePM(),
		Devices: &libvirtxml.DomainDeviceList{
			Emulator:    vm.configureEmulator(),
			Disks:       vm.configureDisks(),
			Controllers: vm.configureControllers(),
			Interfaces:  networkInterfaces,
			Consoles: []libvirtxml.DomainConsole{{
				Source: &libvirtxml.DomainChardevSource{
					Pty: &libvirtxml.DomainChardevSourcePty{},
				},
			}},
			Channels:  vm.configureChannels(),
			Inputs:    []libvirtxml.DomainInput{{Type: "tablet", Bus: "usb"}},
			Graphics:  vm.configureGraphics(),
			Sounds:    vm.configureSounds(),
			Videos:    vm.configureVideos(),
			RedirDevs: vm.configureRedirDevs(),
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
		QEMUCommandline: &libvirtxml.DomainQEMUCommandline{
			Args: qemuExtraArgs,
		},
		SecLabel: secLabels,
	}
}

// isArm64 reports whether the VM targets the ARM64 architecture.
func (vm *VirtDeployVM) isArm64() bool {
	return vm.Arch == "arm64"
}

// configureChannels returns SPICE channels for AMD64 VMs only.
// ARM64's virt machine type does not support SPICE, so no channels are needed.
func (vm *VirtDeployVM) configureChannels() []libvirtxml.DomainChannel {
	if vm.isArm64() {
		return nil
	}
	return []libvirtxml.DomainChannel{{
		Source: &libvirtxml.DomainChardevSource{
			SpiceVMC: &libvirtxml.DomainChardevSourceSpiceVMC{},
		},
		Target: &libvirtxml.DomainChannelTarget{
			VirtIO: &libvirtxml.DomainChannelTargetVirtIO{
				Name: "com.redhat.spice.0",
			},
		},
	}}
}

// configureGraphics returns SPICE graphics for AMD64 VMs only.
// ARM64's virt machine type does not support SPICE graphics.
func (vm *VirtDeployVM) configureGraphics() []libvirtxml.DomainGraphic {
	if vm.isArm64() {
		return nil
	}
	return []libvirtxml.DomainGraphic{{
		Spice: &libvirtxml.DomainGraphicSpice{
			Port:     -1,
			TLSPort:  -1,
			AutoPort: "yes",
			Image: &libvirtxml.DomainGraphicSpiceImage{
				Compression: "off",
			},
		},
	}}
}

// configureSounds returns sound devices for AMD64 VMs only.
// The ich6 sound device is not available on ARM64's virt machine type.
func (vm *VirtDeployVM) configureSounds() []libvirtxml.DomainSound {
	if vm.isArm64() {
		return nil
	}
	return []libvirtxml.DomainSound{{Model: "ich6"}}
}

// configureVideos returns QXL video for AMD64 VMs only.
// The QXL video device is not supported on ARM64's virt machine type.
func (vm *VirtDeployVM) configureVideos() []libvirtxml.DomainVideo {
	if vm.isArm64() {
		return nil
	}
	return []libvirtxml.DomainVideo{{
		Model: libvirtxml.DomainVideoModel{Type: "qxl"},
	}}
}

// configureRedirDevs returns SPICE USB redirect devices for AMD64 VMs only.
// These require SPICE graphics which are not available on ARM64.
func (vm *VirtDeployVM) configureRedirDevs() []libvirtxml.DomainRedirDev {
	if vm.isArm64() {
		return nil
	}
	return []libvirtxml.DomainRedirDev{
		{Bus: "usb", Source: &libvirtxml.DomainChardevSource{
			SpiceVMC: &libvirtxml.DomainChardevSourceSpiceVMC{},
		}},
		{Bus: "usb", Source: &libvirtxml.DomainChardevSource{
			SpiceVMC: &libvirtxml.DomainChardevSourceSpiceVMC{},
		}},
	}
}

// configureDomainType returns the libvirt domain type.
// AMD64 uses KVM (hardware-accelerated). ARM64 uses plain QEMU (TCG) because
// we cross-compile on an x86 host where KVM is not available for aarch64.
func (vm *VirtDeployVM) configureDomainType() string {
	if vm.isArm64() {
		return "qemu"
	}
	return "kvm"
}

// configureOSType returns the OS type element for the domain.
// AMD64 uses the q35 machine type (modern Intel chipset with PCIe).
// ARM64 uses the virt-6.2 machine type (minimal ARM virtual platform).
func (vm *VirtDeployVM) configureOSType() libvirtxml.DomainOSType {
	if vm.isArm64() {
		return libvirtxml.DomainOSType{Arch: "aarch64", Machine: "virt-6.2", Type: "hvm"}
	}
	return libvirtxml.DomainOSType{Arch: "x86_64", Machine: "q35", Type: "hvm"}
}

// configureCPUModel returns the CPU model for the domain.
// AMD64 uses Broadwell-IBRS (widely compatible Intel model with Spectre mitigations).
// ARM64 uses cortex-a57 with fallback=forbid (must match exactly since TCG
// emulation requires a specific ARM core model).
func (vm *VirtDeployVM) configureCPUModel() libvirtxml.DomainCPUModel {
	if vm.isArm64() {
		return libvirtxml.DomainCPUModel{Fallback: "forbid", Value: "cortex-a57"}
	}
	return libvirtxml.DomainCPUModel{Fallback: "allow", Value: "Broadwell-IBRS"}
}

// configureCPUFeatures returns required CPU features.
// AMD64 requires VMX (nested virtualization support).
// ARM64 has no additional CPU feature requirements.
func (vm *VirtDeployVM) configureCPUFeatures() []libvirtxml.DomainCPUFeature {
	if vm.isArm64() {
		return nil
	}
	return []libvirtxml.DomainCPUFeature{{Policy: "require", Name: "vmx"}}
}

// configurePM returns power management settings.
// AMD64 explicitly disables suspend-to-mem and suspend-to-disk.
// ARM64's virt machine does not support ACPI power management.
func (vm *VirtDeployVM) configurePM() *libvirtxml.DomainPM {
	if vm.isArm64() {
		return nil
	}
	return &libvirtxml.DomainPM{
		SuspendToMem:  &libvirtxml.DomainPMPolicy{Enabled: "no"},
		SuspendToDisk: &libvirtxml.DomainPMPolicy{Enabled: "no"},
	}
}

// configureTimers returns clock timer configuration.
// AMD64 configures RTC, PIT, and HPET timers (standard x86 timer stack).
// ARM64 uses the ARM architectural timer (built-in), so no explicit timers are needed.
func (vm *VirtDeployVM) configureTimers() []libvirtxml.DomainTimer {
	if vm.isArm64() {
		return nil
	}
	return []libvirtxml.DomainTimer{
		{Name: "rtc", TickPolicy: "catchup"},
		{Name: "pit", TickPolicy: "delay"},
		{Name: "hpet", Present: "no"},
	}
}

// configureEmulator returns the path to the QEMU binary for the target architecture.
func (vm *VirtDeployVM) configureEmulator() string {
	if vm.isArm64() {
		return "/usr/bin/qemu-system-aarch64"
	}
	return "/usr/bin/qemu-system-x86_64"
}

// configureAPIC returns the APIC feature for AMD64 VMs.
// APIC is an x86-specific interrupt controller; ARM64 uses GIC instead,
// which is implicitly configured by the virt machine type.
func (vm *VirtDeployVM) configureAPIC() *libvirtxml.DomainFeatureAPIC {
	if vm.isArm64() {
		return nil
	}
	return &libvirtxml.DomainFeatureAPIC{}
}

// configureVMPort returns the VMware port feature state.
// Disabled on AMD64 (not needed outside VMware). Not applicable on ARM64.
func (vm *VirtDeployVM) configureVMPort() *libvirtxml.DomainFeatureState {
	if vm.isArm64() {
		return nil
	}
	return &libvirtxml.DomainFeatureState{State: "off"}
}

// configureControllers returns USB controllers for the domain.
// AMD64 uses ich9-ehci1 + three ich9-uhci companions (q35 chipset USB 2.0 stack).
// ARM64 uses a single qemu-xhci controller (USB 3.0, supported by virt machine).
func (vm *VirtDeployVM) configureControllers() []libvirtxml.DomainController {
	if vm.isArm64() {
		return []libvirtxml.DomainController{
			{Type: "usb", Index: new(uint), Model: "qemu-xhci", Alias: &libvirtxml.DomainAlias{Name: "usb"}},
		}
	}
	return []libvirtxml.DomainController{
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
	}
}

// configureDisks returns disk and CDROM devices for the domain.
// AMD64 uses SATA disks (native to q35). ARM64 uses virtio disks because
// SATA + QCOW2 on the virt machine type causes EFI firmware loading failures.
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

		// ARM64 + SATA + QCOW2 seems to have issues: the EFI files are not loaded.
		// Because of this, we use virtio for disks (not CDs) on ARM64
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

// configureTpms returns an emulated TPM 2.0 device if enabled.
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
