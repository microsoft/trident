package virtdeploy

import (
	"fmt"
	"net"
	"reflect"
	"strings"
	"testing"

	"github.com/digitalocean/go-libvirt"
)

// Unimplemented calls panic rather than accidentally reaching a host libvirt.
type ownershipClient struct {
	libvirtClient
	domains       map[string]string
	networks      map[string]string
	pools         map[string]string
	liveDomains   map[string]string
	listPoolError error
	volumes       []libvirt.StorageVol
	inactive      bool
	xmlError      error
	deleted       []string
}

func (c *ownershipClient) ConnectListAllDomains(int32, libvirt.ConnectListAllDomainsFlags) ([]libvirt.Domain, uint32, error) {
	var result []libvirt.Domain
	for name := range c.domains {
		result = append(result, libvirt.Domain{Name: name})
	}
	return result, uint32(len(result)), nil
}

func (c *ownershipClient) ConnectListAllNetworks(int32, libvirt.ConnectListAllNetworksFlags) ([]libvirt.Network, uint32, error) {
	var result []libvirt.Network
	for name := range c.networks {
		result = append(result, libvirt.Network{Name: name})
	}
	return result, uint32(len(result)), nil
}

func (c *ownershipClient) ConnectListAllStoragePools(int32, libvirt.ConnectListAllStoragePoolsFlags) ([]libvirt.StoragePool, uint32, error) {
	if c.listPoolError != nil {
		return nil, 0, c.listPoolError
	}
	var result []libvirt.StoragePool
	for name := range c.pools {
		result = append(result, libvirt.StoragePool{Name: name})
	}
	return result, uint32(len(result)), nil
}

func (c *ownershipClient) DomainGetXMLDesc(dom libvirt.Domain, flags libvirt.DomainXMLFlags) (string, error) {
	if definition, ok := c.liveDomains[dom.Name]; flags == 0 && ok {
		return definition, c.xmlError
	}
	return c.domains[dom.Name], c.xmlError
}

func (c *ownershipClient) NetworkGetXMLDesc(nw libvirt.Network, _ uint32) (string, error) {
	return c.networks[nw.Name], c.xmlError
}

func (c *ownershipClient) StoragePoolGetXMLDesc(pool libvirt.StoragePool, _ libvirt.StorageXMLFlags) (string, error) {
	return c.pools[pool.Name], c.xmlError
}

func (c *ownershipClient) DomainIsActive(dom libvirt.Domain) (int32, error) {
	if _, ok := c.liveDomains[dom.Name]; ok {
		return 1, nil
	}
	return 0, nil
}
func (c *ownershipClient) NetworkIsActive(libvirt.Network) (int32, error) { return 0, nil }
func (c *ownershipClient) StoragePoolIsActive(libvirt.StoragePool) (int32, error) {
	if c.inactive {
		return 0, nil
	}
	return 1, nil
}

func (c *ownershipClient) DomainUndefineFlags(dom libvirt.Domain, _ libvirt.DomainUndefineFlagsValues) error {
	c.deleted = append(c.deleted, "domain:"+dom.Name)
	return nil
}

func (c *ownershipClient) DomainDestroy(dom libvirt.Domain) error {
	c.deleted = append(c.deleted, "stop-domain:"+dom.Name)
	return nil
}

func (c *ownershipClient) NetworkUndefine(nw libvirt.Network) error {
	c.deleted = append(c.deleted, "network:"+nw.Name)
	return nil
}

func (c *ownershipClient) StoragePoolListAllVolumes(libvirt.StoragePool, int32, uint32) ([]libvirt.StorageVol, uint32, error) {
	if c.inactive {
		return nil, 0, fmt.Errorf("pool is inactive")
	}
	return c.volumes, uint32(len(c.volumes)), nil
}

func (c *ownershipClient) StorageVolLookupByName(_ libvirt.StoragePool, name string) (libvirt.StorageVol, error) {
	return libvirt.StorageVol{Name: name}, nil
}

func (c *ownershipClient) StorageVolDelete(vol libvirt.StorageVol, _ libvirt.StorageVolDeleteFlags) error {
	c.deleted = append(c.deleted, "volume:"+vol.Name)
	return nil
}

func (c *ownershipClient) DomainLookupByName(name string) (libvirt.Domain, error) {
	return libvirt.Domain{Name: name}, nil
}

func (c *ownershipClient) NetworkLookupByName(name string) (libvirt.Network, error) {
	return libvirt.Network{Name: name}, nil
}

func (c *ownershipClient) NetworkDefineXML(string) (libvirt.Network, error) {
	c.deleted = append(c.deleted, "define-network")
	return libvirt.Network{}, fmt.Errorf("test stops after network definition")
}

func (c *ownershipClient) StoragePoolDestroy(pool libvirt.StoragePool) error {
	c.deleted = append(c.deleted, "stop:"+pool.Name)
	return nil
}

func (c *ownershipClient) StoragePoolUndefine(pool libvirt.StoragePool) error {
	c.deleted = append(c.deleted, "pool:"+pool.Name)
	return nil
}

func (c *ownershipClient) StoragePoolLookupByName(name string) (libvirt.StoragePool, error) {
	return libvirt.StoragePool{Name: name}, nil
}

func poolDefinition(t *testing.T, name, target string) string {
	t.Helper()
	definition, err := newPool(name, target).asXml()
	if err != nil {
		t.Fatal(err)
	}
	return definition
}

func TestCleanupDoesNotDeleteOverlappingNamespacePool(t *testing.T) {
	for _, test := range []struct {
		clean  namespace
		target string
	}{
		{"foo", "/var/lib/libvirt/foo-nvram"},
		{"foo-nvram", "/var/lib/foo-nvram"},
	} {
		client := &ownershipClient{pools: map[string]string{
			"foo-nvram-pool": poolDefinition(t, "foo-nvram-pool", test.target),
		}}
		rc := &virtDeployResourceConfig{namespace: test.clean, lv: client}
		if err := rc.cleanup(); err == nil {
			t.Fatal("ambiguous pool ownership must abort cleanup")
		}
		if len(client.deleted) != 0 {
			t.Fatalf("clean %s mutated another namespace's pool: %v", test.clean, client.deleted)
		}
	}
}

func TestCleanupUsesResourceTypes(t *testing.T) {
	client := &ownershipClient{
		domains:  map[string]string{"foo-pool": "<domain><name>foo-pool</name></domain>"},
		networks: map[string]string{"foo-vm-0": "<network><name>foo-vm-0</name></network>"},
		pools:    map[string]string{"foo-network": poolDefinition(t, "foo-network", "/var/lib/libvirt/foo")},
	}
	rc := &virtDeployResourceConfig{namespace: "foo", lv: client}
	if err := rc.cleanup(); err != nil {
		t.Fatal(err)
	}
	if len(client.deleted) != 0 {
		t.Fatalf("deleted resources of the wrong type: %v", client.deleted)
	}
}

func TestCleanupLegacyPartialPools(t *testing.T) {
	for _, target := range []struct{ name, path string }{
		{"foo-pool", "/var/lib/libvirt/foo"},
		{"foo-nvram-pool", "/var/lib/foo-nvram"},
	} {
		for _, inactive := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/inactive=%v", target.name, inactive), func(t *testing.T) {
				client := &ownershipClient{
					pools:    map[string]string{target.name: poolDefinition(t, target.name, target.path)},
					inactive: inactive,
				}
				rc := &virtDeployResourceConfig{namespace: "foo", lv: client}
				if err := rc.cleanup(); err != nil {
					t.Fatal(err)
				}
				want := []string{"pool:" + target.name}
				if !inactive {
					want = append([]string{"stop:" + target.name}, want...)
				}
				if !reflect.DeepEqual(client.deleted, want) {
					t.Fatalf("deleted %v, want %v", client.deleted, want)
				}
			})
		}
	}
}

func TestReplacementRefusesUnverifiedPool(t *testing.T) {
	client := &ownershipClient{pools: map[string]string{
		"foo-nvram-pool": poolDefinition(t, "foo-nvram-pool", "/var/lib/libvirt/foo-nvram"),
	}}
	rc := &virtDeployResourceConfig{namespace: "foo", lv: client}
	if err := rc.teardownStoragePoolByName("foo-nvram-pool"); err == nil {
		t.Fatal("replacement must reject an overlapping namespace's pool")
	}
	if len(client.deleted) != 0 {
		t.Fatalf("replacement mutated a foreign pool: %v", client.deleted)
	}
}

func TestCleanupFailsClosedOnUnreadablePoolXML(t *testing.T) {
	for _, xmlError := range []error{nil, fmt.Errorf("denied")} {
		client := &ownershipClient{
			pools: map[string]string{"foo-pool": "<pool"}, xmlError: xmlError,
		}
		rc := &virtDeployResourceConfig{namespace: "foo", lv: client}
		if err := rc.cleanup(); err == nil {
			t.Fatal("expected XML read/parse error")
		}
		if len(client.deleted) != 0 {
			t.Fatalf("mutated pool without verifying XML: %v", client.deleted)
		}
	}
}

func TestCleanupDeletesVolumesOnlyFromVerifiedActivePool(t *testing.T) {
	client := &ownershipClient{
		pools:   map[string]string{"foo-pool": poolDefinition(t, "foo-pool", "/var/lib/libvirt/foo")},
		volumes: []libvirt.StorageVol{{Name: "foo-vm-0-volume-0.qcow2"}},
	}
	rc := &virtDeployResourceConfig{namespace: "foo", lv: client}
	if err := rc.cleanup(); err != nil {
		t.Fatal(err)
	}
	want := []string{"volume:foo-vm-0-volume-0.qcow2", "stop:foo-pool", "pool:foo-pool"}
	if !reflect.DeepEqual(client.deleted, want) {
		t.Fatalf("deleted %v, want %v", client.deleted, want)
	}
}

func generatedDefinitions(t *testing.T, owner namespace) (domain, network string) {
	t.Helper()
	vm := &VirtDeployVM{
		namespace: owner, name: "foo-vm-0", networkName: "foo-network",
		nvramPath: "/var/lib/foo-nvram/foo-vm-0_VARS.fd", Mem: 2, Cpus: 1,
	}
	domain, err := vm.asXml()
	if err != nil {
		t.Fatal(err)
	}
	_, cidr, err := net.ParseCIDR("192.0.2.0/24")
	if err != nil {
		t.Fatal(err)
	}
	nw, err := newVirtDeployNetwork("foo-network", *cidr, "")
	if err != nil {
		t.Fatal(err)
	}
	nw.namespace = owner
	network, err = nw.asXml()
	if err != nil {
		t.Fatal(err)
	}
	return domain, network
}

func TestCleanupGeneratedAndLegacyDomainsAndNetworks(t *testing.T) {
	for _, owner := range []namespace{"", "foo"} {
		t.Run(fmt.Sprintf("metadata=%v", owner != ""), func(t *testing.T) {
			domain, network := generatedDefinitions(t, owner)
			if owner != "" && (!strings.Contains(domain, "urn:trident:virtdeploy:1") ||
				!strings.Contains(network, "urn:trident:virtdeploy:1")) {
				t.Fatal("new resources must include ownership metadata")
			}
			client := &ownershipClient{
				domains:  map[string]string{"foo-vm-0": domain},
				networks: map[string]string{"foo-network": network},
			}
			rc := &virtDeployResourceConfig{namespace: "foo", lv: client}
			if err := rc.cleanup(); err != nil {
				t.Fatal(err)
			}
			want := []string{"domain:foo-vm-0", "network:foo-network"}
			if !reflect.DeepEqual(client.deleted, want) {
				t.Fatalf("deleted %v, want %v", client.deleted, want)
			}
		})
	}
}

func TestCleanupAndReplacementRetainUnverifiedDomainsAndNetworks(t *testing.T) {
	domain, network := generatedDefinitions(t, "foo")
	legacyDomain, legacyNetwork := generatedDefinitions(t, "")
	for name, definitions := range map[string][2]string{
		"foreign metadata": {
			strings.ReplaceAll(domain, `namespace="foo"`, `namespace="foo-nvram"`),
			strings.ReplaceAll(network, `namespace="foo"`, `namespace="foo-nvram"`),
		},
		"wrong kind": {
			strings.ReplaceAll(domain, `kind="domain"`, `kind="network"`),
			strings.ReplaceAll(network, `kind="network"`, `kind="domain"`),
		},
		"unsafe legacy paths and shape": {
			strings.ReplaceAll(legacyDomain, "/var/lib/foo-nvram/", "/var/lib/other/"),
			strings.ReplaceAll(legacyNetwork, `mode="nat"`, `mode="route"`),
		},
		"unsafe marked NVRAM path": {
			strings.ReplaceAll(domain, "/var/lib/foo-nvram/", "/var/lib/other/"),
			strings.ReplaceAll(network, `namespace="foo"`, `namespace="other"`),
		},
		"unmarked namesakes": {
			strings.ReplaceAll(legacyDomain, "virtdeploy:1", "other-tool"),
			"<network><name>foo-network</name></network>",
		},
	} {
		t.Run(name, func(t *testing.T) {
			client := &ownershipClient{
				domains:  map[string]string{"foo-vm-0": definitions[0]},
				networks: map[string]string{"foo-network": definitions[1]},
			}
			rc := &virtDeployResourceConfig{namespace: "foo", lv: client}
			if err := rc.cleanup(); err == nil {
				t.Fatal("unverified domain/network must abort cleanup")
			}
			if err := rc.teardownDomainByName("foo-vm-0"); err == nil {
				t.Error("replacement must reject an unverified domain")
			}
			if err := rc.teardownNetworkByName("foo-network"); err == nil {
				t.Error("replacement must reject an unverified network")
			}
			if len(client.deleted) != 0 {
				t.Fatalf("mutated unverified resources: %v", client.deleted)
			}
		})
	}
}

func TestCleanupRetainsUnverifiedPools(t *testing.T) {
	valid := poolDefinition(t, "foo-pool", "/var/lib/libvirt/foo")
	for _, definition := range []string{
		strings.ReplaceAll(valid, `type="dir"`, `type="fs"`),
		strings.ReplaceAll(valid, "/var/lib/libvirt/foo", "/var/lib/libvirt/other"),
		"<pool type='dir'><name>foo-pool</name></pool>",
	} {
		client := &ownershipClient{pools: map[string]string{"foo-pool": definition}}
		rc := &virtDeployResourceConfig{namespace: "foo", lv: client}
		if err := rc.cleanup(); err == nil {
			t.Fatal("unverified pool must abort cleanup")
		}
		if err := rc.teardownStoragePoolByName("foo-pool"); err == nil {
			t.Error("replacement must reject an unverified pool")
		}
		if len(client.deleted) != 0 {
			t.Fatalf("mutated unverified pool: %v", client.deleted)
		}
	}
}

func TestOwnershipMetadataResolvesNamespacesAndRejectsConflicts(t *testing.T) {
	ns := namespace("foo")
	for _, root := range []string{"domain", "network"} {
		for _, owner := range []string{"foo", "other"} {
			definition := fmt.Sprintf(
				`<%s xmlns:vd="urn:trident:virtdeploy:1"><metadata><vd:owner namespace="%s" kind="%s"/></metadata></%s>`,
				root, owner, root, root,
			)
			present, owned, err := ns.metadataOwnership(definition, root)
			if err != nil || !present || owned != (owner == "foo") {
				t.Fatalf("metadata %s: present=%v owned=%v err=%v", definition, present, owned, err)
			}
		}
	}
	definition := `<domain><metadata xmlns:vd="urn:trident:virtdeploy:1">
		<vd:owner namespace="foo" kind="domain"/>
		<vd:owner namespace="other" kind="domain"/>
	</metadata></domain>`
	present, owned, err := ns.metadataOwnership(definition, "domain")
	if err != nil || !present || owned {
		t.Fatalf("conflicting metadata: present=%v owned=%v err=%v", present, owned, err)
	}
}

func TestOwnershipPreflightPreservesAllResources(t *testing.T) {
	for _, operation := range []string{"cleanup", "replacement"} {
		for _, invalid := range []string{
			"domain", "network", "pool", "pool XML", "retained domain",
			"retained disk", "retained live disk", "retained bridge",
			"retained XML", "extra unverified domain", "pool alias", "inventory error",
		} {
			t.Run(operation+"/"+invalid, func(t *testing.T) {
				domain, network := generatedDefinitions(t, "foo")
				client := &ownershipClient{
					domains:  map[string]string{"foo-vm-0": domain},
					networks: map[string]string{"foo-network": network},
					pools: map[string]string{
						"foo-pool":       poolDefinition(t, "foo-pool", "/var/lib/libvirt/foo"),
						"foo-nvram-pool": poolDefinition(t, "foo-nvram-pool", "/var/lib/foo-nvram"),
					},
					volumes: []libvirt.StorageVol{{Name: "foo-vm-0-volume-0.qcow2"}},
				}
				switch invalid {
				case "domain":
					client.domains["foo-vm-0"] = strings.ReplaceAll(domain, "/var/lib/foo-nvram/", "/var/lib/other/")
				case "network":
					client.networks["foo-network"] = strings.ReplaceAll(network, `namespace="foo"`, `namespace="other"`)
				case "pool":
					client.pools["foo-nvram-pool"] = poolDefinition(t, "foo-nvram-pool", "/var/lib/other")
				case "pool XML":
					client.pools["foo-nvram-pool"] = "<pool"
				case "retained domain":
					client.domains["other-vm-0"] = strings.ReplaceAll(domain, "foo-vm-0", "other-vm-0")
				case "retained disk":
					client.domains["other"] = `<domain><devices><disk type="file"><source file="/var/lib/libvirt/foo/kept.qcow2"/></disk></devices></domain>`
				case "retained live disk":
					client.domains["other"] = `<domain><name>other</name></domain>`
					client.liveDomains = map[string]string{
						"other": `<domain><devices><disk type="file"><source file="/var/lib/libvirt/foo/kept.qcow2"/></disk></devices></domain>`,
					}
				case "retained bridge":
					client.networks["foo-network"] = strings.Replace(network, "<name>", `<bridge name="virbr42"/><name>`, 1)
					client.domains["other"] = `<domain><devices><interface type="bridge"><source bridge="virbr42"/></interface></devices></domain>`
				case "retained XML":
					client.domains["other"] = ""
				case "extra unverified domain":
					client.domains["foo-vm-1"] = strings.ReplaceAll(
						strings.ReplaceAll(domain, "foo-vm-0", "foo-vm-1"),
						"/var/lib/foo-nvram/", "/var/lib/other/",
					)
				case "pool alias":
					client.pools["other"] = poolDefinition(t, "other", "/var/lib/libvirt/foo")
				case "inventory error":
					client.listPoolError = fmt.Errorf("inventory denied")
				}
				_, cidr, err := net.ParseCIDR("192.0.2.0/24")
				if err != nil {
					t.Fatal(err)
				}
				nw, err := newVirtDeployNetwork("foo-network", *cidr, "")
				if err != nil {
					t.Fatal(err)
				}
				rc := &virtDeployResourceConfig{
					namespace: "foo", lv: client, network: nw,
					pool:      newPool("foo-pool", "/var/lib/libvirt/foo"),
					nvramPool: newPool("foo-nvram-pool", "/var/lib/foo-nvram"),
					vms:       []VirtDeployVM{{name: "foo-vm-0"}},
				}
				if operation == "cleanup" {
					err = rc.cleanup()
				} else {
					_, err = rc.construct()
				}
				if err == nil {
					t.Error("unresolved ownership/dependency must abort visibly")
				} else if !strings.Contains(err.Error(), "ownership preflight") {
					t.Errorf("failure must identify ownership preflight: %v", err)
				}
				if len(client.deleted) != 0 {
					t.Fatalf("mutated resources before resolving %s: %v", invalid, client.deleted)
				}
			})
		}
	}
}
