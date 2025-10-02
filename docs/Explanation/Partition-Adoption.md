
# Partition Adoption

When Trident installs or services an operating system, there are times when
existing partitions need to be preserved in the target OS. To accomplish this,
these partitions can be adopted. To adopt a partition, the Trident Host
Configuration can be modified to include details that help Trident identify the
desired partitions. Either (but not both) a label or UUID can be specified.

> Note: Partitions must adhere to some
> [partition type requirements](../Reference/Host-Configuration/Storage-Rules.md)
> to be adopted.

