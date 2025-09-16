# Multiboot

<!-- TODO(8068): Write multiboot docs.-->
<!-- tl;dr for the first PR
Glossary:
Install: a full deployment of an Azure Linux made with trident. 
  We do not really care about other OSes or distros.
  The install encompasses the entire OS, including the bootloader,
  the kernel, the initramfs, the root filesystem, associated partitions,
  and any other partitions that are part of the install.
  This means that an install contains both A and B statuses for 
  deployments that use A/B updates.

Multiboot: The capability of having multiple installs on the same device, 
  even on the same disk.

Install Index: A number that identifies a specific install on a machine.

ESP Directory Name: A string that identifies the combination of a specific 
  install and a specific A/B volume on that install.
  It is used to uniquely name entries in the ESP filesystem.
 -->