package virtdeploy

import (
	"net"
	"reflect"
	"strings"
	"testing"
)

func TestPreflightAllowsEmptyAndUnrelatedNamespaces(t *testing.T) {
	for _, replacing := range []bool{false, true} {
		for _, populated := range []bool{false, true} {
			client := &ownershipClient{}
			if populated {
				client.domains = map[string]string{
					"other-vm-0": `<domain><name>other-vm-0</name><devices><disk type="file"><source file="/var/lib/libvirt/other/disk.qcow2"/></disk></devices></domain>`,
				}
				client.networks = map[string]string{"other-network": "<network/>"}
				client.pools = map[string]string{
					"other-pool": poolDefinition(t, "other-pool", "/var/lib/libvirt/other"),
				}
			}
			rc := &virtDeployResourceConfig{namespace: "foo", lv: client}
			inventory, err := rc.preflight(replacing)
			if err != nil {
				t.Fatalf("replacing=%v populated=%v: %v", replacing, populated, err)
			}
			if len(inventory.domains)+len(inventory.networks)+len(inventory.pools) != 0 ||
				len(client.deleted) != 0 {
				t.Fatal("unrelated resources must not be selected or mutated")
			}
		}
	}
}

func TestReplacementStopsDomainsBeforeReplacingDependencies(t *testing.T) {
	domain, network := generatedDefinitions(t, "foo")
	client := &ownershipClient{
		domains:     map[string]string{"foo-vm-0": domain},
		liveDomains: map[string]string{"foo-vm-0": domain},
		networks:    map[string]string{"foo-network": network},
	}
	nw, err := newVirtDeployNetwork("foo-network", net.IPNet{
		IP: net.IPv4(192, 0, 2, 0), Mask: net.CIDRMask(24, 32),
	}, "")
	if err != nil {
		t.Fatal(err)
	}
	rc := &virtDeployResourceConfig{
		namespace: "foo", lv: client, network: nw,
		vms: []VirtDeployVM{{name: "foo-vm-0"}},
	}
	_, err = rc.construct()
	if err == nil || !strings.Contains(err.Error(), "test stops after network definition") {
		t.Fatalf("expected test client to stop construction: %v", err)
	}
	want := []string{"stop-domain:foo-vm-0", "domain:foo-vm-0", "network:foo-network", "define-network"}
	if !reflect.DeepEqual(client.deleted, want) {
		t.Fatalf("mutation order %v, want %v", client.deleted, want)
	}
}

func TestReplacementRefusesToRetainAdditionalNamespaceDomain(t *testing.T) {
	domain, _ := generatedDefinitions(t, "foo")
	client := &ownershipClient{domains: map[string]string{
		"foo-vm-0": domain,
		"foo-vm-1": strings.ReplaceAll(domain, "foo-vm-0", "foo-vm-1"),
	}}
	rc := &virtDeployResourceConfig{
		namespace: "foo", lv: client,
		vms: []VirtDeployVM{{name: "foo-vm-0"}},
	}
	inventory, err := rc.preflight(false)
	if err != nil || len(inventory.domains) != 2 {
		t.Fatalf("cleanup should accept both owned domains: %v", err)
	}
	_, err = rc.construct()
	if err == nil || !strings.Contains(err.Error(), "would retain domain foo-vm-1") {
		t.Fatalf("replacement must reject retaining a dependent domain: %v", err)
	}
	if len(client.deleted) != 0 {
		t.Fatalf("replacement mutated resources: %v", client.deleted)
	}
}
