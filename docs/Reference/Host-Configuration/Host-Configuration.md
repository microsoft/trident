---
sidebar_position: 0
---

# Host Configuration

Host Configuration is the primary interface for describing the desired state
of a host to Trident. It is a single YAML document that captures everything
Trident needs to know to provision or update a machine: disk layout,
partitioning, filesystems, RAID, encryption, A/B update volumes, OS image
sourcing, users, services, and more.

Trident is declarative: rather than issuing a sequence of imperative
commands, you describe the state you want the host to end up in, and Trident
computes and performs the steps required to get there.

## Where to start

- **[API Reference](./API-Reference/HostConfiguration.md)** — the full,
  auto-generated reference for every field in the Host Configuration schema,
  starting from the top-level `HostConfiguration` type.
- **[Sample Host Configuration](./Sample-Host-Configuration.md)** — a
  complete, annotated example showing RAID, encryption, and A/B update
  configured together.
- **[Storage Configuration Rules](./Storage-Rules.md)** — the validation
  rules Trident applies to the `storage` section, such as reference
  validity, homogeneity requirements, and allowed partition types.

## Structure at a glance

A Host Configuration document is organized into a handful of top-level
sections:

| Section        | Purpose                                                        |
| -------------- | --------------------------------------------------------------|
| `storage`      | Disks, partitions, RAID, encryption, filesystems, mount points |
| `os`           | Target OS configuration (users, services, SELinux, etc.)       |
| `image`        | Sourcing and integrity information for the OS image            |
| `managementOs` | OS configuration used only during clean install servicing      |
| `scripts`      | Scripts to run after Trident servicing stages                  |
| `health`       | Health checks for the target OS                                |

See the [API Reference](./API-Reference/HostConfiguration.md) for the
authoritative, complete list of fields and their types.

## Validation

Trident validates a Host Configuration against a JSON Schema before using
it, and reports any errors it finds. You can check a document's syntax and
structure ahead of time, without applying it to a host, using the `validate`
verb:

```bash
trident validate /path/to/host-configuration.yaml
```

This only validates the file itself. When Trident actually runs an install
or update, it performs additional validation against the target host's
hardware and current state — see [Host Configuration
Validation](../../Explanation/Host-Configuration-Validation.md) for details.

## How Trident uses it

The same Host Configuration document is used for both of Trident's main
verbs:

- **`trident install`** — performs a clean install of the target OS image
  described by `image` onto the storage described in `storage`, using
  `managementOs` for the provisioning environment.
- **`trident update`** — compares the new Host Configuration against the
  host's current state and automatically selects the least disruptive
  servicing type that can apply the change: a
  [runtime update](../../Reference/Glossary.md#runtime-update) (no reboot),
  an [A/B update](../../Reference/Glossary.md#ab-update) (switches to the
  other root partition and reboots), or reports that a clean install is
  required if the change can't be applied in place.

See [How Trident Knows What to
Do](../../Explanation/How-Trident-Knows-What-to-Do.md) and [A/B
Update](../../Explanation/AB-Update.md) for a deeper explanation of this
decision process.

## Related topics

- [Host Configuration Validation](../../Explanation/Host-Configuration-Validation.md)
- [How Trident Knows What to Do](../../Explanation/How-Trident-Knows-What-to-Do.md)
- [A/B Update](../../Explanation/AB-Update.md)
- [Partition Sizes](../../Explanation/Partition-Sizes.md)
- [Script Hooks](../../Explanation/Script-Hooks.md)
