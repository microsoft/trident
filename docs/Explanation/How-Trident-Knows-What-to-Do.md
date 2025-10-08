
# How Trident Knows What to Do

<!--
DELETE ME AFTER COMPLETING THE DOCUMENT!
---
Task: https://dev.azure.com/mariner-org/polar/_workitems/edit/13173
Title: How Trident Knows What to Do
Type: Explanation
Objective:

Explain how trident knows what to do.
-->

The Trident CLI implements several verbs, like `install` and `update`. These
verbs allow Trident to explicitly understand what the user wants to do. For
a given verb, Trident will determine what needs to be done based on the
[Host Configuration](../Reference/Host-Configuration/API-Reference/HostConfiguration.md)
file, which is a declarative API that describing the desired state of the
machine.

The Host Configuration contains a description of the machine's disks,
partitions, filesystems, and other details. It will be compared to the existing
state of the machine to find the differences and determine what actions need to
be taken to bring the machine to the desired state.

Several Trident [subsystems](../Reference/Glossary.md#subsystem) have been
implemented to handle different aspects of the configuration. Each subsystem
is responsible for a specific area (for example,
[storage](../Reference/Host-Configuration/API-Reference/Storage.md)).

Each subsystem will validate the relevant Host Configuration setting(s),
prepare the system for changes, perform the required changes, and then
modify any required system configurations. The various subsystems in the
can be seen in the [Install Flow](../Explanation/Install-Flow.md) diagram.

Using `storage` as an example, Trident will read the Host Configuration and
determine how to get to the desired state. This includes things like:

* adopting and creating disk partitions
* configuring A/B volume pairs
* deploying filesystems
* setting up encryption
* creating swap and raid arrays
