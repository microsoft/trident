package virtdeploy

import (
	"errors"
	"reflect"
	"strings"
	"testing"

	"github.com/digitalocean/go-libvirt"
)

type namespaceCleanupStub struct {
	domains     []libvirt.Domain
	networks    []libvirt.Network
	pools       []libvirt.StoragePool
	definitions map[string]string
	readPools   []string
	deleted     []string
	failAt      string
	failure     error
}

func (c *namespaceCleanupStub) fail(operation string) error {
	if c.failAt == operation {
		return c.failure
	}
	return nil
}

func (c *namespaceCleanupStub) ConnectListAllDomains(int32, libvirt.ConnectListAllDomainsFlags) ([]libvirt.Domain, uint32, error) {
	return c.domains, uint32(len(c.domains)), c.fail("list domains")
}

func (c *namespaceCleanupStub) ConnectListAllNetworks(int32, libvirt.ConnectListAllNetworksFlags) ([]libvirt.Network, uint32, error) {
	return c.networks, uint32(len(c.networks)), c.fail("list networks")
}

func (c *namespaceCleanupStub) ConnectListAllStoragePools(int32, libvirt.ConnectListAllStoragePoolsFlags) ([]libvirt.StoragePool, uint32, error) {
	return c.pools, uint32(len(c.pools)), c.fail("list storage pools")
}

func (c *namespaceCleanupStub) StoragePoolGetXMLDesc(pool libvirt.StoragePool, _ libvirt.StorageXMLFlags) (string, error) {
	c.readPools = append(c.readPools, pool.Name)
	return c.definitions[pool.Name], c.fail("read " + pool.Name)
}

func (c *namespaceCleanupStub) teardown(kind, name string) error {
	operation := kind + " " + name
	c.deleted = append(c.deleted, operation)
	return c.fail(operation)
}

func (c *namespaceCleanupStub) cleanup(ns namespace) error {
	return cleanupNamespaceResources(ns, c,
		func(dom libvirt.Domain) error { return c.teardown("domain", dom.Name) },
		func(nw libvirt.Network) error { return c.teardown("network", nw.Name) },
		func(pool libvirt.StoragePool) error { return c.teardown("pool", pool.Name) },
	)
}

func cleanupPoolXML(t *testing.T, name, target string) string {
	t.Helper()
	definition, err := newPool(name, target).asXml()
	if err != nil {
		t.Fatal(err)
	}
	return definition
}

func TestCleanupNamespaceSelectsCanonicalResources(t *testing.T) {
	for _, ns := range []namespace{"foo", "foo-nvram", "virtdeploy", "trident-e2e-example", ""} {
		t.Run(ns.String(), func(t *testing.T) {
			client := &namespaceCleanupStub{
				domains: []libvirt.Domain{
					{Name: ns.vmName(0)}, {Name: ns.vmName(42)},
					{Name: ns.libvirtNetworkName()}, {Name: ns.storagePoolName()},
					{Name: ns.String() + "-vm-01"}, {Name: ns.String() + "-vm-+1"},
				},
				networks: []libvirt.Network{
					{Name: ns.libvirtNetworkName()}, {Name: ns.vmName(0)}, {Name: ns.storagePoolName()},
				},
				pools: []libvirt.StoragePool{
					{Name: ns.storagePoolName()}, {Name: ns.nvramPoolName()},
					{Name: ns.libvirtNetworkName()}, {Name: ns.vmName(0)},
				},
				definitions: map[string]string{
					ns.storagePoolName(): cleanupPoolXML(t, ns.storagePoolName(), "/var/lib/libvirt/"+ns.String()),
					ns.nvramPoolName():   cleanupPoolXML(t, ns.nvramPoolName(), "/var/lib/"+ns.String()+"-nvram"),
				},
			}
			for _, other := range []namespace{namespace(ns.String() + "bar"), namespace(ns.String() + "-bar"), namespace(ns.vmName(1))} {
				client.domains = append(client.domains, libvirt.Domain{Name: other.vmName(0)})
				client.networks = append(client.networks, libvirt.Network{Name: other.libvirtNetworkName()})
				client.pools = append(client.pools, libvirt.StoragePool{Name: other.storagePoolName()}, libvirt.StoragePool{Name: other.nvramPoolName()})
			}
			if err := client.cleanup(ns); err != nil {
				t.Fatal(err)
			}
			want := []string{
				"domain " + ns.vmName(0), "domain " + ns.vmName(42),
				"network " + ns.libvirtNetworkName(),
				"pool " + ns.storagePoolName(), "pool " + ns.nvramPoolName(),
			}
			if !reflect.DeepEqual(client.deleted, want) {
				t.Fatalf("deleted %v, want %v", client.deleted, want)
			}
			if !reflect.DeepEqual(client.readPools, []string{ns.storagePoolName(), ns.nvramPoolName()}) {
				t.Fatalf("inspected unrelated pools: %v", client.readPools)
			}
		})
	}
}

func TestCleanupNamespaceRejectsPoolConflictsBeforeDeletion(t *testing.T) {
	for _, tt := range []struct {
		name       string
		ns         namespace
		pool       string
		definition string
	}{
		{"sibling disk pool", "foo", "foo-nvram-pool", cleanupPoolXML(t, "foo-nvram-pool", "/var/lib/libvirt/foo-nvram")},
		{"sibling nvram pool", "foo-nvram", "foo-nvram-pool", cleanupPoolXML(t, "foo-nvram-pool", "/var/lib/foo-nvram")},
		{"wrong backend", "foo", "foo-pool", `<pool type="fs"><name>foo-pool</name><target><path>/var/lib/libvirt/foo</path></target></pool>`},
		{"wrong target", "foo", "foo-pool", cleanupPoolXML(t, "foo-pool", "/somewhere/else")},
		{"wrong definition name", "foo", "foo-pool", cleanupPoolXML(t, "other-pool", "/var/lib/libvirt/foo")},
		{"missing target", "foo", "foo-pool", `<pool type="dir"><name>foo-pool</name></pool>`},
		{"malformed XML", "foo", "foo-pool", `<pool`},
	} {
		t.Run(tt.name, func(t *testing.T) {
			// A valid candidate precedes the conflict, and domains/networks exist.
			// None may be deleted before all candidate pools have been validated.
			validPool := tt.ns.nvramPoolName()
			if validPool == tt.pool {
				validPool = tt.ns.storagePoolName()
			}
			target, _ := tt.ns.poolTarget(validPool)
			client := &namespaceCleanupStub{
				domains:  []libvirt.Domain{{Name: tt.ns.vmName(0)}},
				networks: []libvirt.Network{{Name: tt.ns.libvirtNetworkName()}},
				pools:    []libvirt.StoragePool{{Name: validPool}, {Name: tt.pool}},
				definitions: map[string]string{
					validPool: cleanupPoolXML(t, validPool, target),
					tt.pool:   tt.definition,
				},
			}
			err := client.cleanup(tt.ns)
			if err == nil || !strings.Contains(err.Error(), tt.pool) {
				t.Fatalf("expected visible conflict for %s, got %v", tt.pool, err)
			}
			if len(client.deleted) != 0 {
				t.Fatalf("deleted resources before rejecting conflict: %v", client.deleted)
			}
		})
	}
}

func TestCleanupNamespacePropagatesErrors(t *testing.T) {
	for _, operation := range []string{
		"list domains", "list networks", "list storage pools", "read foo-pool",
		"domain foo-vm-0", "network foo-network", "pool foo-pool",
	} {
		t.Run(operation, func(t *testing.T) {
			failure := errors.New("injected failure")
			client := &namespaceCleanupStub{
				domains:     []libvirt.Domain{{Name: "foo-vm-0"}},
				networks:    []libvirt.Network{{Name: "foo-network"}},
				pools:       []libvirt.StoragePool{{Name: "foo-pool"}},
				definitions: map[string]string{"foo-pool": cleanupPoolXML(t, "foo-pool", "/var/lib/libvirt/foo")},
				failAt:      operation,
				failure:     failure,
			}
			if err := client.cleanup("foo"); !errors.Is(err, failure) {
				t.Fatalf("expected propagated failure, got %v", err)
			}
			wantDeleted := []string(nil)
			mutations := []string{"domain foo-vm-0", "network foo-network", "pool foo-pool"}
			for i, mutation := range mutations {
				if operation == mutation {
					wantDeleted = mutations[:i+1]
				}
			}
			if !reflect.DeepEqual(client.deleted, wantDeleted) {
				t.Fatalf("deleted %v after failure, want %v", client.deleted, wantDeleted)
			}
		})
	}
}

func TestCleanupNamespaceWithNoSelectedResources(t *testing.T) {
	ns := namespace("foo")
	for _, name := range []string{"empty inventory", "unrelated prefix resources"} {
		t.Run(name, func(t *testing.T) {
			client := &namespaceCleanupStub{}
			if name == "unrelated prefix resources" {
				client.domains = []libvirt.Domain{
					{Name: "foo-bar-vm-0"},
					{Name: "foobar-vm-0"},
				}
				client.networks = []libvirt.Network{
					{Name: "foo-bar-network"},
					{Name: "foobar-network"},
				}
				client.pools = []libvirt.StoragePool{
					{Name: "foo-bar-pool"},
					{Name: "foo-bar-nvram-pool"},
					{Name: "foobar-pool"},
					{Name: "foobar-nvram-pool"},
				}
				// Unrelated definitions must not even be read.
				client.definitions = map[string]string{
					"foo-bar-pool":      `<pool type="invalid"><broken`,
					"foobar-nvram-pool": `{invalid json}`,
				}
			}
			if err := client.cleanup(ns); err != nil {
				t.Fatal(err)
			}
			if len(client.deleted) != 0 || len(client.readPools) != 0 {
				t.Fatalf("expected no cleanup; deleted=%v, readPools=%v", client.deleted, client.readPools)
			}
		})
	}
}
