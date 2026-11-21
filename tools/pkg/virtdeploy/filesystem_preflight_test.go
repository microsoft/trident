package virtdeploy

import (
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/digitalocean/go-libvirt"
	"libvirt.org/go/libvirtxml"
)

// Map only the two namespace directories into disposable storage. Symlinks and
// inode checks otherwise use the real local filesystem, never host libvirt.
type mappedResourceFilesystem struct {
	root string
}

func (f mappedResourceFilesystem) mapped(value string) string {
	for name, root := range map[string]string{
		"/var/lib/libvirt/foo": "disks",
		"/var/lib/foo-nvram":   "nvram",
	} {
		if value == name || strings.HasPrefix(value, name+"/") {
			return filepath.Join(f.root, root) + strings.TrimPrefix(value, name)
		}
	}
	return value
}

func (f mappedResourceFilesystem) resolve(value string) (filesystemPath, error) {
	return (osResourceFilesystem{}).resolve(f.mapped(value))
}

func (f mappedResourceFilesystem) readDir(value string) ([]os.DirEntry, error) {
	return os.ReadDir(f.mapped(value))
}

func filesystemPreflightConfig(t *testing.T) (*virtDeployResourceConfig, *ownershipClient, mappedResourceFilesystem) {
	t.Helper()
	files := mappedResourceFilesystem{root: t.TempDir()}
	for _, directory := range []string{"disks", "nvram"} {
		if err := os.Mkdir(filepath.Join(files.root, directory), 0755); err != nil {
			t.Fatal(err)
		}
	}
	domain, network := generatedDefinitions(t, "foo")
	client := &ownershipClient{
		domains:  map[string]string{"foo-vm-0": domain},
		networks: map[string]string{"foo-network": network},
		pools: map[string]string{
			"foo-pool":       poolDefinition(t, "foo-pool", "/var/lib/libvirt/foo"),
			"foo-nvram-pool": poolDefinition(t, "foo-nvram-pool", "/var/lib/foo-nvram"),
		},
	}
	nw, err := newVirtDeployNetwork("foo-network", net.IPNet{
		IP: net.IPv4(192, 0, 2, 0), Mask: net.CIDRMask(24, 32),
	}, "")
	if err != nil {
		t.Fatal(err)
	}
	rc := &virtDeployResourceConfig{
		namespace: "foo", lv: client, files: files, network: nw,
		vms: []VirtDeployVM{{name: "foo-vm-0"}},
	}
	return rc, client, files
}

func TestRetainedPoolProtectsStandaloneNVRAMDeletionTarget(t *testing.T) {
	for _, structured := range []bool{false, true} {
		t.Run(fmt.Sprintf("structured=%v", structured), func(t *testing.T) {
			rc, client, files := filesystemPreflightConfig(t)
			const target = "/var/lib/foo-nvram/foo-vm-0_VARS.fd"
			if err := os.WriteFile(files.mapped(target), []byte("preserve"), 0600); err != nil {
				t.Fatal(err)
			}
			if structured {
				client.domains["foo-vm-0"] = nvramDomainDefinition(t, libvirtxml.DomainNVRam{
					Source: &libvirtxml.DomainDiskSource{
						File: &libvirtxml.DomainDiskSourceFile{File: target},
					},
				})
			}
			client.pools = map[string]string{
				"retained": poolDefinition(t, "retained", "/var/lib/foo-nvram"),
			}
			if err := rc.cleanup(); err == nil || !strings.Contains(err.Error(), "retained storage pool") {
				t.Errorf("pool exposing standalone NVRAM must block deletion: %v", err)
			}
			if len(client.deleted) != 0 {
				t.Fatalf("mutated resources: %v", client.deleted)
			}
		})
	}
}

func TestPreflightResolvesRetainedFilesystemAliases(t *testing.T) {
	for _, operation := range []string{"cleanup", "replacement"} {
		for _, kind := range []string{"domain symlink", "pool symlink", "target symlink", "unresolved symlink"} {
			t.Run(operation+"/"+kind, func(t *testing.T) {
				rc, client, files := filesystemPreflightConfig(t)
				disk := filepath.Join(files.root, "disks", "disk.qcow2")
				if err := os.WriteFile(disk, []byte("preserve"), 0600); err != nil {
					t.Fatal(err)
				}
				alias := filepath.Join(files.root, "kept.qcow2")
				switch kind {
				case "domain symlink":
					if err := os.Symlink(disk, alias); err != nil {
						t.Fatal(err)
					}
				case "pool symlink":
					if err := os.Symlink(filepath.Dir(disk), alias); err != nil {
						t.Fatal(err)
					}
					client.pools["retained"] = poolDefinition(t, "retained", alias)
				case "target symlink":
					actual := filepath.Join(files.root, "actual")
					if err := os.Rename(filepath.Dir(disk), actual); err != nil {
						t.Fatal(err)
					}
					if err := os.Symlink(actual, filepath.Dir(disk)); err != nil {
						t.Fatal(err)
					}
					alias = filepath.Join(actual, "disk.qcow2")
				case "unresolved symlink":
					if err := os.Symlink(alias, alias); err != nil {
						t.Fatal(err)
					}
				}
				if kind != "pool symlink" {
					client.domains["other"] = fmt.Sprintf(
						`<domain><name>other</name><devices><disk type="file"><source file="%s"/></disk></devices></domain>`, alias,
					)
				}
				var err error
				if operation == "cleanup" {
					err = rc.cleanup()
				} else {
					_, err = rc.construct()
				}
				if err == nil || !strings.Contains(err.Error(), "ownership preflight") {
					t.Errorf("filesystem alias/identity uncertainty must abort preflight: %v", err)
				}
				if len(client.deleted) != 0 {
					t.Fatalf("mutated resources before resolving identities: %v", client.deleted)
				}
			})
		}
	}
}

func TestReplacementRejectsUnverifiedRetainedDirectoryFiles(t *testing.T) {
	for _, registered := range []bool{false, true} {
		for _, name := range []string{"foo-pool", "foo-nvram-pool"} {
			t.Run(fmt.Sprintf("%s/registered=%v", name, registered), func(t *testing.T) {
				rc, client, files := filesystemPreflightConfig(t)
				client.domains = nil
				client.inactive = true
				if !registered {
					client.pools = nil
				}
				directory := "disks"
				if name == "foo-nvram-pool" {
					directory = "nvram"
				}
				sentinel := filepath.Join(files.root, directory, "retained-volume")
				if err := os.WriteFile(sentinel, []byte("preserve"), 0600); err != nil {
					t.Fatal(err)
				}
				_, err := rc.construct()
				if err == nil || !strings.Contains(err.Error(), "unverified files") {
					t.Errorf("replacement must reject retained files before mutation: %v", err)
				}
				if len(client.deleted) != 0 {
					t.Fatalf("mutated resources: %v", client.deleted)
				}
				content, err := os.ReadFile(sentinel)
				if err != nil || string(content) != "preserve" {
					t.Fatalf("retained file changed: %q, %v", content, err)
				}
			})
		}
	}
}

func TestDependencyChecksIgnoreNonReferenceDomainText(t *testing.T) {
	rc, client, _ := filesystemPreflightConfig(t)
	client.domains["foo-pool"] = `<domain>
		<name>foo-pool</name><description>foo-network</description>
		<metadata><note file="/var/lib/libvirt/foo/disk.qcow2">foo-nvram-pool</note></metadata>
		<devices><filesystem type="mount"><source dir="/unrelated/share"/>
			<target dir="/var/lib/libvirt/foo/guest-path"/></filesystem></devices>
	</domain>`
	if _, err := rc.preflight(false); err != nil {
		t.Fatalf("names, descriptions and arbitrary metadata are not dependencies: %v", err)
	}
	if len(client.deleted) != 0 {
		t.Fatalf("preflight mutated resources: %v", client.deleted)
	}
}

func TestDependencyChecksUseActualReferenceFields(t *testing.T) {
	for name, reference := range map[string]string{
		"volume pool": `<devices><disk type="volume"><source pool="foo-pool" volume="disk"/></disk></devices>`,
		"network":     `<devices><interface type="network"><source network="foo-network"/></interface></devices>`,
		"backing file": `<devices><disk type="file"><source file="/unrelated/image"/>
			<backingStore type="file"><source file="/var/lib/libvirt/foo/base"/></backingStore></disk></devices>`,
		"nested backing file": `<devices><disk type="file"><backingStore type="file">
			<source file="/unrelated/base"/><backingStore type="file"><source file="/var/lib/libvirt/foo/base"/>
			</backingStore></backingStore></disk></devices>`,
		"mirror file":    `<devices><disk type="file"><mirror type="file"><source file="/var/lib/libvirt/foo/mirror"/></mirror></disk></devices>`,
		"filesystem":     `<devices><filesystem type="mount"><source dir="/var/lib/libvirt/foo/share"/></filesystem></devices>`,
		"NVRAM template": `<os><nvram template="/var/lib/foo-nvram/foo-vm-0_VARS.fd">/unrelated/vars</nvram></os>`,
		"fwcfg":          `<sysinfo type="fwcfg"><entry name="config" file="/var/lib/libvirt/foo/config"/></sysinfo>`,
		"serial file":    `<devices><serial type="file"><source path="/var/lib/libvirt/foo/serial"/></serial></devices>`,
		"channel socket": `<devices><channel type="unix"><source path="/var/lib/libvirt/foo/channel" mode="bind"/></channel></devices>`,
		"memory backing": `<devices><memory model="nvdimm"><source><path>/var/lib/libvirt/foo/memory</path></source></memory></devices>`,
		"TPM storage":    `<devices><tpm model="tpm-crb"><backend type="emulator"><source type="dir" path="/var/lib/libvirt/foo/tpm"/></backend></tpm></devices>`,
		"lease file":     `<devices><lease><target path="/var/lib/libvirt/foo/lease"/></lease></devices>`,
		"RNG file":       `<devices><rng model="virtio"><backend model="random">/var/lib/libvirt/foo/rng</backend></rng></devices>`,
		"shared memory":  `<devices><shmem name="shared"><server path="/var/lib/libvirt/foo/shared"/></shmem></devices>`,
		"QEMU ignition": `<qemu:commandline xmlns:qemu="http://libvirt.org/schemas/domain/qemu/1.0">
			<qemu:arg value="-fw_cfg"/><qemu:arg value="name=opt/org.flatcar-linux/config,file=/var/lib/libvirt/foo/config"/>
			</qemu:commandline>`,
	} {
		t.Run(name, func(t *testing.T) {
			rc, client, _ := filesystemPreflightConfig(t)
			client.domains["other"] = "<domain><name>other</name>" + reference + "</domain>"
			if err := rc.cleanup(); err == nil || !strings.Contains(err.Error(), "retained domain other") {
				t.Errorf("actual dependency must block deletion: %v", err)
			}
			if len(client.deleted) != 0 {
				t.Fatalf("mutated resources: %v", client.deleted)
			}
		})
	}
}

func TestReplacementAcceptsEmptyOrManagedActiveDirectories(t *testing.T) {
	for _, state := range []string{"missing", "empty undefined", "empty inactive", "populated active"} {
		t.Run(state, func(t *testing.T) {
			rc, client, files := filesystemPreflightConfig(t)
			switch state {
			case "missing":
				for _, directory := range []string{"disks", "nvram"} {
					if err := os.Remove(filepath.Join(files.root, directory)); err != nil {
						t.Fatal(err)
					}
				}
				client.pools = nil
			case "empty undefined":
				client.pools = nil
			case "empty inactive":
				client.inactive = true
			case "populated active":
				for _, directory := range []string{"disks", "nvram"} {
					if err := os.WriteFile(filepath.Join(files.root, directory, "managed-volume"), []byte("managed"), 0600); err != nil {
						t.Fatal(err)
					}
				}
			}
			if _, err := rc.preflight(true); err != nil {
				t.Fatalf("safe existing replacement behavior must remain available: %v", err)
			}
		})
	}
}

func TestInactiveCleanupDoesNotAuthorizeRetainedFilesOnRecreation(t *testing.T) {
	rc, client, files := filesystemPreflightConfig(t)
	client.domains = nil
	client.inactive = true
	sentinel := filepath.Join(files.root, "disks", "foo-vm-0-volume-0.qcow2")
	if err := os.WriteFile(sentinel, []byte("preserve"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := rc.cleanup(); err != nil {
		t.Fatal(err)
	}
	for _, mutation := range client.deleted {
		if strings.HasPrefix(mutation, "volume:") {
			t.Fatalf("inactive cleanup deleted a retained volume: %v", client.deleted)
		}
	}
	client.pools = nil
	client.networks = nil
	client.deleted = nil
	if _, err := rc.construct(); err == nil || !strings.Contains(err.Error(), "unverified files") {
		t.Errorf("undefined pool must not authorize old volume files on recreation: %v", err)
	}
	if len(client.deleted) != 0 {
		t.Fatalf("recreation mutated resources: %v", client.deleted)
	}
	content, err := os.ReadFile(sentinel)
	if err != nil || string(content) != "preserve" {
		t.Fatalf("retained file changed across cleanup/recreation: %q, %v", content, err)
	}
}

func TestPreflightPreservesLexicalAndTraversedDeletionPaths(t *testing.T) {
	for _, operation := range []string{"cleanup", "replacement"} {
		for _, kind := range []string{"outgoing symlink", "parent traversal", "escaped traversal", "alias chain"} {
			t.Run(operation+"/"+kind, func(t *testing.T) {
				rc, client, files := filesystemPreflightConfig(t)
				outside := filepath.Join(files.root, "outside")
				if err := os.MkdirAll(filepath.Join(outside, "nested"), 0755); err != nil {
					t.Fatal(err)
				}
				disk := filepath.Join(outside, "disk.qcow2")
				if err := os.WriteFile(disk, []byte("preserve"), 0600); err != nil {
					t.Fatal(err)
				}
				const managed = "/var/lib/libvirt/foo"
				link := managed + "/link.qcow2"
				linkTarget := disk
				reference := link
				switch kind {
				case "parent traversal":
					linkTarget = filepath.Join(outside, "nested")
					reference = link + "/../disk.qcow2"
				case "escaped traversal":
					linkTarget = filepath.Join(outside, "nested")
					reference = link + "/../../outside/disk.qcow2"
				case "alias chain":
					reference = filepath.Join(files.root, "retained.qcow2")
					if err := os.Symlink(files.mapped(link), reference); err != nil {
						t.Fatal(err)
					}
				}
				if err := os.Symlink(linkTarget, files.mapped(link)); err != nil {
					t.Fatal(err)
				}
				client.volumes = []libvirt.StorageVol{{Name: "link.qcow2"}}
				client.domains["other"] = fmt.Sprintf(
					`<domain><name>other</name><devices><disk type="file"><source file="%s"/></disk></devices></domain>`, reference,
				)
				var err error
				if operation == "cleanup" {
					err = rc.cleanup()
				} else {
					_, err = rc.construct()
				}
				if err == nil || !strings.Contains(err.Error(), "retained domain other") {
					t.Errorf("deleting a traversed symlink must abort preflight: %v", err)
				}
				if len(client.deleted) != 0 {
					t.Fatalf("mutated resources with a retained lexical dependency: %v", client.deleted)
				}
			})
		}
	}
}

func TestPreflightProtectsFilesystemBinaryAndGraphicsNetwork(t *testing.T) {
	for _, operation := range []string{"cleanup", "replacement"} {
		for name, reference := range map[string]string{
			"virtiofs executable": `<filesystem type="mount"><driver type="virtiofs"/>
				<binary path="/var/lib/libvirt/foo/virtiofsd"/><source dir="/unrelated/share"/>
				<target dir="shared"/></filesystem>`,
			"binary without source": `<filesystem type="mount"><binary path="/var/lib/libvirt/foo/virtiofsd"/></filesystem>`,
			"VNC network":           `<graphics type="vnc"><listen type="network" network="foo-network"/></graphics>`,
			"SPICE network":         `<graphics type="spice"><listen type="network" network="foo-network"/></graphics>`,
			"RDP network":           `<graphics type="rdp"><listen type="network" network="foo-network"/></graphics>`,
		} {
			t.Run(operation+"/"+name, func(t *testing.T) {
				rc, client, files := filesystemPreflightConfig(t)
				if err := os.WriteFile(files.mapped("/var/lib/libvirt/foo/virtiofsd"), []byte("preserve"), 0700); err != nil {
					t.Fatal(err)
				}
				client.volumes = []libvirt.StorageVol{{Name: "virtiofsd"}}
				client.domains["other"] = "<domain><name>other</name><devices>" + reference + "</devices></domain>"
				var err error
				if operation == "cleanup" {
					err = rc.cleanup()
				} else {
					_, err = rc.construct()
				}
				if err == nil || !strings.Contains(err.Error(), "retained domain other") {
					t.Errorf("actual binary/network dependency must abort preflight: %v", err)
				}
				if len(client.deleted) != 0 {
					t.Fatalf("mutated resources with a retained binary/network dependency: %v", client.deleted)
				}
			})
		}
	}
}

func TestPreflightAllowsUnrelatedFilesystemBinaryAndGraphicsNetwork(t *testing.T) {
	for _, replacement := range []bool{false, true} {
		t.Run(fmt.Sprintf("replacement=%v", replacement), func(t *testing.T) {
			rc, client, files := filesystemPreflightConfig(t)
			binary := filepath.Join(files.root, "unrelated-virtiofsd")
			if err := os.WriteFile(binary, []byte("preserve"), 0700); err != nil {
				t.Fatal(err)
			}
			client.domains["other"] = fmt.Sprintf(`<domain><name>other</name><devices>
				<filesystem type="mount"><binary path="%s"/><source dir="/unrelated/share"/>
					<target dir="/var/lib/libvirt/foo/guest-path"/></filesystem>
				<graphics type="vnc"><listen type="network" network="unrelated-network"/></graphics>
				</devices></domain>`, binary)
			if _, err := rc.preflight(replacement); err != nil {
				t.Fatalf("unrelated binary/network and guest paths must not block preflight: %v", err)
			}
			if len(client.deleted) != 0 {
				t.Fatalf("preflight mutated resources: %v", client.deleted)
			}
		})
	}
}
