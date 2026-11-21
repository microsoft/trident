package virtdeploy

import (
	"fmt"
	"path"
	"strings"

	"github.com/digitalocean/go-libvirt"
	"libvirt.org/go/libvirtxml"
)

type resourceInventory struct {
	domains  []libvirt.Domain
	networks []libvirt.Network
	pools    []libvirt.StoragePool
}

func verifyOwnership(name, definition string, owns func(string, string) (bool, error)) error {
	owned, err := owns(name, definition)
	if err != nil {
		return fmt.Errorf("verify %s ownership: %w", name, err)
	}
	if !owned {
		return fmt.Errorf("%w: %s", errResourceNotOwned, name)
	}
	return nil
}

// Preflight is read-only. No resource may be destroyed until the entire
// inventory, including retained domains' dependencies, has been checked.
func (rc *virtDeployResourceConfig) preflight(replacing bool) (*resourceInventory, error) {
	domains, _, err := rc.lv.ConnectListAllDomains(1, libvirt.ConnectListDomainsActive|libvirt.ConnectListDomainsInactive)
	if err != nil {
		return nil, fmt.Errorf("list domains: %w", err)
	}
	networks, _, err := rc.lv.ConnectListAllNetworks(1, libvirt.ConnectListNetworksActive|libvirt.ConnectListNetworksInactive)
	if err != nil {
		return nil, fmt.Errorf("list networks: %w", err)
	}
	pools, _, err := rc.lv.ConnectListAllStoragePools(1, libvirt.ConnectListStoragePoolsActive|libvirt.ConnectListStoragePoolsInactive)
	if err != nil {
		return nil, fmt.Errorf("list storage pools: %w", err)
	}

	ns := rc.namespace
	files := rc.files
	if files == nil {
		files = osResourceFilesystem{}
	}
	inventory := &resourceInventory{}
	targets := resourceTargets{
		networks: map[string]bool{},
		bridges:  map[string]bool{},
		pools:    map[string]bool{},
	}
	activePools := map[string]bool{}
	if replacing {
		targets.networks[ns.libvirtNetworkName()] = true
		targets.pools[ns.storagePoolName()] = true
		targets.pools[ns.nvramPoolName()] = true
	}
	for _, nw := range networks {
		if nw.Name == ns.libvirtNetworkName() {
			targets.networks[nw.Name] = true
			definition, err := rc.lv.NetworkGetXMLDesc(nw, 0)
			if err != nil {
				return nil, fmt.Errorf("get network %s XML: %w", nw.Name, err)
			}
			if err := verifyOwnership(nw.Name, definition, ns.ownsNetwork); err != nil {
				return nil, err
			}
			var parsed libvirtxml.Network
			if err := parsed.Unmarshal(definition); err != nil {
				return nil, fmt.Errorf("inspect network %s: %w", nw.Name, err)
			}
			if parsed.Bridge != nil && parsed.Bridge.Name != "" {
				targets.bridges[parsed.Bridge.Name] = true
			}
			inventory.networks = append(inventory.networks, nw)
		}
	}
	for _, pool := range pools {
		if pool.Name == ns.storagePoolName() || pool.Name == ns.nvramPoolName() {
			targets.pools[pool.Name] = true
			definition, err := rc.lv.StoragePoolGetXMLDesc(pool, 0)
			if err != nil {
				return nil, fmt.Errorf("get storage pool %s XML: %w", pool.Name, err)
			}
			if err := verifyOwnership(pool.Name, definition, ns.ownsPool); err != nil {
				return nil, err
			}
			if replacing {
				active, err := rc.lv.StoragePoolIsActive(pool)
				if err != nil {
					return nil, fmt.Errorf("check storage pool %s activity: %w", pool.Name, err)
				}
				activePools[pool.Name] = active != 0
			}
			inventory.pools = append(inventory.pools, pool)
		}
	}
	for name, root := range map[string]string{
		ns.storagePoolName(): ns.storagePoolPath(),
		ns.nvramPoolName():   ns.nvramPoolPath(),
	} {
		if targets.pools[name] {
			if err := targets.addPath(root, files); err != nil {
				return nil, err
			}
		}
	}
	if replacing {
		if err := rc.verifyReplacementDirectories(files, activePools); err != nil {
			return nil, err
		}
	}

	// UndefineNvram unlinks the writable NVRAM source independently of pools.
	// Collect every selected domain's target before checking retained domains.
	for _, dom := range domains {
		if !ns.ownsDomainName(dom.Name) {
			continue
		}
		definition, err := rc.lv.DomainGetXMLDesc(dom, libvirt.DomainXMLInactive)
		if err != nil {
			return nil, fmt.Errorf("get domain %s XML: %w", dom.Name, err)
		}
		if err := verifyOwnership(dom.Name, definition, ns.ownsDomain); err != nil {
			return nil, err
		}
		var parsed libvirtxml.Domain
		if err := parsed.Unmarshal(definition); err != nil {
			return nil, fmt.Errorf("inspect domain %s NVRAM: %w", dom.Name, err)
		}
		if err := targets.addPath(domainNVRAMPath(&parsed), files); err != nil {
			return nil, err
		}
	}

	for _, dom := range domains {
		candidate := ns.ownsDomainName(dom.Name)
		if candidate && replacing {
			requested := false
			for _, vm := range rc.vms {
				requested = requested || vm.name == dom.Name
			}
			if !requested {
				return nil, fmt.Errorf("replacement would retain domain %s while replacing its namespace resources", dom.Name)
			}
		}
		flags := []libvirt.DomainXMLFlags{libvirt.DomainXMLInactive}
		active, err := rc.lv.DomainIsActive(dom)
		if err != nil {
			return nil, fmt.Errorf("check domain %s activity: %w", dom.Name, err)
		}
		if active != 0 {
			flags = append(flags, 0)
		}
		for _, flag := range flags {
			definition, err := rc.lv.DomainGetXMLDesc(dom, flag)
			if err != nil {
				return nil, fmt.Errorf("get domain %s XML: %w", dom.Name, err)
			}
			if candidate {
				if err := verifyOwnership(dom.Name, definition, ns.ownsDomain); err != nil {
					return nil, err
				}
			} else {
				dependent, err := targets.domainReferences(definition, files)
				if err != nil {
					return nil, fmt.Errorf("inspect retained domain %s dependencies: %w", dom.Name, err)
				}
				if dependent {
					return nil, fmt.Errorf("retained domain %s depends on namespace %s resources", dom.Name, ns)
				}
			}
		}
		if candidate {
			inventory.domains = append(inventory.domains, dom)
		}
	}

	// Another pool can expose the same files under a different name.
	for _, pool := range pools {
		if pool.Name == ns.storagePoolName() || pool.Name == ns.nvramPoolName() {
			continue
		}
		definition, err := rc.lv.StoragePoolGetXMLDesc(pool, 0)
		if err != nil {
			return nil, fmt.Errorf("get storage pool %s XML: %w", pool.Name, err)
		}
		var parsed libvirtxml.StoragePool
		if err := parsed.Unmarshal(definition); err != nil {
			return nil, fmt.Errorf("inspect storage pool %s: %w", pool.Name, err)
		}
		if parsed.Target != nil {
			overlaps, err := targets.overlapsPath(parsed.Target.Path, files)
			if err != nil {
				return nil, fmt.Errorf("inspect retained storage pool %s: %w", pool.Name, err)
			}
			if overlaps {
				return nil, fmt.Errorf("retained storage pool %s overlaps namespace %s storage", pool.Name, ns)
			}
		}
	}
	return inventory, nil
}

func pathsOverlap(a, b string) bool {
	if a == "" || b == "" {
		return false
	}
	a, b = path.Clean(a), path.Clean(b)
	return a == b || a == "/" || b == "/" ||
		strings.HasPrefix(a, b+"/") || strings.HasPrefix(b, a+"/")
}
