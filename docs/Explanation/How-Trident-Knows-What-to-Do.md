
# How Trident Knows What to Do

The Trident CLI implements several verbs, like `install` and `update`. These
verbs allow Trident to explicitly understand what the user wants to do. For a
given verb, Trident will determine what needs to be done based on any flags
specified in the command and the [Host
Configuration](../Reference/Host-Configuration/API-Reference/HostConfiguration.md)
file, which is a declarative API that describes the desired state of the
machine.

## Install Command

For `install`, a [Clean Install](../Reference/Glossary.md#clean-install) is
typically triggered. If the `--multiboot` flag is used, multiple operating
systems may be installed. See the [Explanation of
Multiboot](../Explanation/Multiboot.md) for more information.

## Update Command

Trident automatically selects the appropriate servicing type between an [A/B
update](../Reference/Glossary.md#ab-update) and a [runtime
update](../Reference/Glossary.md#runtime-update) based on which configurations
have changed in the Host Configuration. If only [sysexts, confexts, or netplan
configuration](../Reference/Glossary.md#runtime-update) have changed, Trident
will perform a runtime update. Otherwise, Trident will trigger an A/B update to
provision a new root filesystem.

## Trident Subsystems

The Host Configuration contains a description of the machine's disks,
partitions, filesystems, and other details. It will be compared to the existing
state of the machine to find the differences and determine what actions need to
be taken to bring the machine to the desired state.

Several Trident [subsystems](../Reference/Glossary.md#subsystem) have been
implemented to handle different aspects of the Host Configuration. Each
subsystem is responsible for a specific area (for example,
[storage](../Reference/Host-Configuration/API-Reference/Storage.md)).

Each subsystem will validate the relevant Host Configuration setting(s), prepare
the system for changes, perform the required changes, and then modify any
required system configurations. The various subsystems in the Trident
architecture can be seen in the [Install Flow](../Explanation/Install-Flow.md)
diagram.

Using `storage` as an example, Trident will read the Host Configuration and
determine how to get to the desired state. This includes things like:

* adopting and creating disk partitions
* configuring A/B volume pairs
* deploying filesystems
* setting up encryption
* creating swap and raid arrays
