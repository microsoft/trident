package virtdeploy

import (
	"os"
	"path"
	"strings"

	"libvirt.org/go/libvirtxml"
)

type resourceTargets struct {
	networks map[string]bool
	bridges  map[string]bool
	pools    map[string]bool
	paths    []filesystemPath
}

func (targets *resourceTargets) addPath(value string, files resourceFilesystem) error {
	if value == "" {
		return nil
	}
	resolved, err := files.resolve(value)
	if err != nil {
		return err
	}
	resolved.lexical = value
	targets.paths = append(targets.paths, resolved)
	return nil
}

func (targets *resourceTargets) overlapsPath(value string, files resourceFilesystem) (bool, error) {
	if value == "" || len(targets.paths) == 0 {
		return false, nil
	}
	resolved, err := files.resolve(value)
	if err != nil {
		return false, err
	}
	for _, target := range targets.paths {
		if pathsOverlap(value, target.lexical) || pathsOverlap(resolved.path, target.path) ||
			(resolved.info != nil && target.info != nil && os.SameFile(resolved.info, target.info)) {
			return true, nil
		}
		for _, traversed := range resolved.traversed {
			if pathWithin(traversed, target.lexical) || pathWithin(traversed, target.path) {
				return true, nil
			}
		}
	}
	return false, nil
}

func pathWithin(value, root string) bool {
	if value == "" || root == "" {
		return false
	}
	value, root = path.Clean(value), path.Clean(root)
	return value == root || strings.HasPrefix(value, strings.TrimRight(root, "/")+"/")
}

func (targets *resourceTargets) domainReferences(definition string, files resourceFilesystem) (bool, error) {
	var domain libvirtxml.Domain
	if err := domain.Unmarshal(definition); err != nil {
		return false, err
	}
	var paths []string
	var pools []string
	var source func(*libvirtxml.DomainDiskSource)
	source = func(s *libvirtxml.DomainDiskSource) {
		if s == nil {
			return
		}
		if s.File != nil {
			paths = append(paths, s.File.File)
		}
		if s.Block != nil {
			paths = append(paths, s.Block.Dev)
		}
		if s.Dir != nil {
			paths = append(paths, s.Dir.Dir)
		}
		if s.Volume != nil {
			pools = append(pools, s.Volume.Pool)
		}
		if s.VHostUser != nil {
			paths = append(paths, characterDevicePaths((*libvirtxml.DomainChardevSource)(s.VHostUser))...)
		}
		if s.VHostVDPA != nil {
			paths = append(paths, s.VHostVDPA.Dev)
		}
		if s.Network != nil {
			for _, host := range s.Network.Hosts {
				paths = append(paths, host.Socket)
			}
			if s.Network.KnownHosts != nil {
				paths = append(paths, s.Network.KnownHosts.Path)
			}
			if s.Network.Config != nil {
				paths = append(paths, s.Network.Config.File)
			}
			if s.Network.Identity != nil {
				paths = append(paths, s.Network.Identity.Keyfile, s.Network.Identity.AgentSock)
			}
		}
		if s.DataStore != nil {
			source(s.DataStore.Source)
		}
	}
	backing := func(b *libvirtxml.DomainDiskBackingStore) {
		for ; b != nil; b = b.BackingStore {
			source(b.Source)
		}
	}
	if domain.OS != nil {
		paths = append(paths, domain.OS.Kernel, domain.OS.Initrd, domain.OS.Shim, domain.OS.DTB)
		if domain.OS.Loader != nil {
			paths = append(paths, domain.OS.Loader.Path)
		}
		if domain.OS.ACPI != nil {
			for _, table := range domain.OS.ACPI.Tables {
				paths = append(paths, table.Path)
			}
		}
		if nvram := domain.OS.NVRam; nvram != nil {
			paths = append(paths, nvram.Template, domainNVRAMPath(&domain))
			source(nvram.Source)
		}
	}
	if domain.Devices != nil {
		paths = append(paths, deviceHostPaths(domain.Devices)...)
		for _, disk := range domain.Devices.Disks {
			source(disk.Source)
			backing(disk.BackingStore)
			if disk.Mirror != nil {
				source(disk.Mirror.Source)
				backing(disk.Mirror.BackingStore)
			}
		}
		for _, filesystem := range domain.Devices.Filesystems {
			if filesystem.Binary != nil {
				paths = append(paths, filesystem.Binary.Path)
			}
			s := filesystem.Source
			if s == nil {
				continue
			}
			if s.Mount != nil {
				paths = append(paths, s.Mount.Dir, s.Mount.Socket)
			}
			if s.Block != nil {
				paths = append(paths, s.Block.Dev)
			}
			if s.File != nil {
				paths = append(paths, s.File.File)
			}
			if s.Bind != nil {
				paths = append(paths, s.Bind.Dir)
			}
			if s.Volume != nil {
				pools = append(pools, s.Volume.Pool)
			}
		}
		for _, iface := range domain.Devices.Interfaces {
			s := iface.Source
			if s == nil {
				continue
			}
			if s.Network != nil && (targets.networks[s.Network.Network] || targets.bridges[s.Network.Bridge]) {
				return true, nil
			}
			if s.Bridge != nil && targets.bridges[s.Bridge.Bridge] {
				return true, nil
			}
			if s.Direct != nil && targets.bridges[s.Direct.Dev] {
				return true, nil
			}
			if s.VHostUser != nil {
				paths = append(paths, characterDevicePaths(s.VHostUser.Chardev)...)
			}
		}
		for _, graphics := range domain.Devices.Graphics {
			var listeners []libvirtxml.DomainGraphicListener
			if graphics.VNC != nil {
				listeners = append(listeners, graphics.VNC.Listeners...)
			}
			if graphics.Spice != nil {
				listeners = append(listeners, graphics.Spice.Listeners...)
			}
			if graphics.RDP != nil {
				listeners = append(listeners, graphics.RDP.Listeners...)
			}
			for _, listener := range listeners {
				if listener.Network != nil && targets.networks[listener.Network.Network] {
					return true, nil
				}
			}
		}
	}
	for _, sysinfo := range domain.SysInfo {
		if sysinfo.FWCfg != nil {
			for _, entry := range sysinfo.FWCfg.Entry {
				paths = append(paths, entry.File)
			}
		}
	}
	if domain.QEMUCommandline != nil {
		for _, arg := range domain.QEMUCommandline.Args {
			// Generated ignition uses -fw_cfg name=...,file=<host path>.
			for _, option := range strings.Split(arg.Value, ",") {
				if value, ok := strings.CutPrefix(option, "file="); ok {
					paths = append(paths, value)
				}
			}
		}
	}
	for _, pool := range pools {
		if targets.pools[pool] {
			return true, nil
		}
	}
	for _, value := range paths {
		overlaps, err := targets.overlapsPath(value, files)
		if overlaps || err != nil {
			return overlaps, err
		}
	}
	return false, nil
}

func characterDevicePaths(source *libvirtxml.DomainChardevSource) []string {
	if source == nil {
		return nil
	}
	var paths []string
	if source.File != nil {
		paths = append(paths, source.File.Path)
	}
	if source.Dev != nil {
		paths = append(paths, source.Dev.Path)
	}
	if source.Pty != nil {
		paths = append(paths, source.Pty.Path)
	}
	if source.Pipe != nil {
		paths = append(paths, source.Pipe.Path)
	}
	if source.UNIX != nil {
		paths = append(paths, source.UNIX.Path)
	}
	return paths
}

func deviceHostPaths(devices *libvirtxml.DomainDeviceList) []string {
	paths := []string{devices.Emulator}
	character := func(source *libvirtxml.DomainChardevSource, log *libvirtxml.DomainChardevLog) {
		paths = append(paths, characterDevicePaths(source)...)
		if log != nil {
			paths = append(paths, log.File)
		}
	}
	for _, serial := range devices.Serials {
		character(serial.Source, serial.Log)
	}
	for _, parallel := range devices.Parallels {
		character(parallel.Source, parallel.Log)
	}
	for _, console := range devices.Consoles {
		character(console.Source, console.Log)
	}
	for _, channel := range devices.Channels {
		character(channel.Source, channel.Log)
	}
	for _, redir := range devices.RedirDevs {
		character(redir.Source, nil)
	}
	for _, card := range devices.Smartcards {
		character(card.Passthrough, nil)
		paths = append(paths, card.Database)
		for _, cert := range card.HostCerts {
			paths = append(paths, cert.File)
		}
	}
	for _, memory := range devices.Memorydevs {
		if memory.Source != nil {
			paths = append(paths, memory.Source.Path)
		}
	}
	for _, lease := range devices.Leases {
		if lease.Target != nil {
			paths = append(paths, lease.Target.Path)
		}
	}
	for _, shared := range devices.Shmems {
		if shared.Server != nil {
			paths = append(paths, shared.Server.Path)
		}
	}
	for _, rng := range devices.RNGs {
		if rng.Backend != nil {
			if rng.Backend.Random != nil {
				paths = append(paths, rng.Backend.Random.Device)
			}
			if rng.Backend.EGD != nil {
				character(rng.Backend.EGD.Source, nil)
			}
		}
	}
	for _, tpm := range devices.TPMs {
		if b := tpm.Backend; b != nil {
			if b.Passthrough != nil && b.Passthrough.Device != nil {
				paths = append(paths, b.Passthrough.Device.Path)
			}
			if b.External != nil {
				character((*libvirtxml.DomainChardevSource)(b.External.Source), nil)
			}
			if b.Emulator != nil && b.Emulator.Source != nil {
				if b.Emulator.Source.File != nil {
					paths = append(paths, b.Emulator.Source.File.Path)
				}
				if b.Emulator.Source.Dir != nil {
					paths = append(paths, b.Emulator.Source.Dir.Path)
				}
			}
		}
	}
	if devices.PStore != nil {
		paths = append(paths, devices.PStore.Path)
	}
	return paths
}
