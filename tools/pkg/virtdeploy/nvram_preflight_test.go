package virtdeploy

import (
	"fmt"
	"reflect"
	"strings"
	"testing"

	"github.com/digitalocean/go-libvirt"
	"libvirt.org/go/libvirtxml"
)

type orderedNVRAMClient struct {
	*ownershipClient
	retainedFirst bool
}

func (c *orderedNVRAMClient) ConnectListAllDomains(int32, libvirt.ConnectListAllDomainsFlags) ([]libvirt.Domain, uint32, error) {
	domains := []libvirt.Domain{{Name: "foo-vm-0"}, {Name: "other"}}
	if c.retainedFirst {
		domains[0], domains[1] = domains[1], domains[0]
	}
	return domains, 2, nil
}

func nvramDomainDefinition(t *testing.T, nvram libvirtxml.DomainNVRam) string {
	t.Helper()
	definition, _ := generatedDefinitions(t, "foo")
	var domain libvirtxml.Domain
	if err := domain.Unmarshal(definition); err != nil {
		t.Fatal(err)
	}
	domain.OS.NVRam = &nvram
	definition, err := domain.Marshal()
	if err != nil {
		t.Fatal(err)
	}
	return definition
}

func TestCleanupPreflightsDomainNVRAMWithoutPool(t *testing.T) {
	const target = "/var/lib/foo-nvram/foo-vm-0_VARS.fd"
	const template = "/usr/share/OVMF/OVMF_VARS_4M.fd"
	for _, source := range []bool{false, true} {
		nvram := libvirtxml.DomainNVRam{NVRam: target, Template: template}
		if source {
			nvram.NVRam = ""
			nvram.Source = &libvirtxml.DomainDiskSource{
				File: &libvirtxml.DomainDiskSourceFile{File: target},
			}
		}
		selected := nvramDomainDefinition(t, nvram)
		for _, reference := range []string{
			"<os><nvram>" + target + "</nvram></os>",
			`<os><nvram type="file"><source file="` + target + `"/></nvram></os>`,
			`<devices><disk type="file"><source file="/var/lib/foo-nvram/./foo-vm-0_VARS.fd"/></disk></devices>`,
			"<os><nvram template=\"" + template + "\">/var/lib/other_VARS.fd</nvram></os>",
		} {
			for _, retainedFirst := range []bool{false, true} {
				t.Run(fmt.Sprintf("source=%v/retainedFirst=%v/%s", source, retainedFirst, reference), func(t *testing.T) {
					client := &orderedNVRAMClient{
						ownershipClient: &ownershipClient{domains: map[string]string{
							"foo-vm-0": selected,
							"other":    "<domain><name>other</name>" + reference + "</domain>",
						}},
						retainedFirst: retainedFirst,
					}
					rc := &virtDeployResourceConfig{namespace: "foo", lv: client}
					err := rc.cleanup()
					if strings.Contains(reference, target) || strings.Contains(reference, "/./") {
						if err == nil || !strings.Contains(err.Error(), "retained domain other") {
							t.Fatalf("shared writable NVRAM must abort preflight: %v", err)
						}
						if len(client.deleted) != 0 {
							t.Fatalf("mutated resources with shared NVRAM and no pool: %v", client.deleted)
						}
					} else {
						if err != nil {
							t.Fatal(err)
						}
						if !reflect.DeepEqual(client.deleted, []string{"domain:foo-vm-0"}) {
							t.Fatalf("template is not a deletion target: %v", client.deleted)
						}
					}
				})
			}
		}
	}
}

func TestDomainNVRAMOwnershipDoesNotUseTemplateOrShadowedText(t *testing.T) {
	const expected = "/var/lib/foo-nvram/foo-vm-0_VARS.fd"
	for _, nvram := range []libvirtxml.DomainNVRam{
		{NVRam: "/var/lib/other.fd", Template: expected},
		{
			NVRam: expected, Template: expected,
			Source: &libvirtxml.DomainDiskSource{
				File: &libvirtxml.DomainDiskSourceFile{File: "/var/lib/other.fd"},
			},
		},
	} {
		owned, err := namespace("foo").ownsDomain("foo-vm-0", nvramDomainDefinition(t, nvram))
		if err != nil {
			t.Fatal(err)
		}
		if owned {
			t.Fatal("template or shadowed text must not establish ownership of a foreign NVRAM source")
		}
	}
}
