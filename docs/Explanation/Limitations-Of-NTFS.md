# Limitations of NTFS

## Background

NTFS, or New Technology File System, is a file system developed by Microsoft
for storing and retrieving files on Windows operating systems. NTFS offers
features such as security descriptors, encryption, rich metadata, and so on.

## NTFS User Space Driver

To support the NTFS file system on Linux, the Linux community has developed
multiple projects, including NTFS-3G. NTFS-3G uses FUSE, or "Filesystem in
Userspace," which is a Linux kernel module that allows users to create their
own custom file systems without needing to modify the kernel code. Thus,
NTFS-3G uses a user-space driver, which is a kernel component that interacts
with hardware devices but which, unlike a kernel driver, runs in the user
space.

### Limitations

Because NTFS in Linux is mostly supported in the user space, using NTFS in the
Linux environment via this solution comes with certain limitations, such as
limited memory, reduced performance, and constraints around mounting.

#### NTFS and Multiboot

One particular limitation that is relevant to Trident's customers is related to
**using NTFS in the multiboot flow**.

Let's imagine that the customer requests the host to have an NTFS partition,
which should be shared between multiple operating systems. In this case,
Trident will mount the NTFS partition during each OS installation. However,
**NTFS does not support multiple mounts**.

To solve this problem, Trident will create **a private bind mount** from the
existing mount point to the new desired mount point, instead of a "regular"
mount. Customers who wish to use NTFS partitions in their multiboot flow
should be aware of the fact that the partition will be mounted using a private
bind mount, unlike other file system types.

#### NTFS and SELinux

Another limitation is that NTFS **cannot be used in conjunction with SELinux**.
The NTFS file system does not inhenerently support the security labeling system
that SELinux relies on, and so SELinux cannot assign security contexts to the
files placed on NTFS partitions.

To solve this incompatibility, Trident will **skip the `setfiles` operation**
for NTFS. Normally, `setfiles` is used to initialize the security context
labels on files within the file system, during the SELinux installation
process. For NTFS file systems, this operation will not be run.
