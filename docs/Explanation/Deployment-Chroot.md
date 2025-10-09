
# Deployment Chroot

When Trident is deploying a target OS, it will mount the target OS's file
systems and prepare them for a chroot. This mount of the target OS is called
`newroot`.

[chroot](https://www.linux.org/docs/man1/chroot.html) is a Unix operation that
changes the apparent root directory for the current running process and its
children. A program that is run in such a modified environment cannot access
files outside the designated directory tree.

Trident will then chroot into the `newroot` and run commands in the context of
the target OS. This allows Trident to perform tasks such as installing the boot
loader, configuring the network, and other tasks that require running commands
in the context of the target OS.

Trident uses `chroot` to change the root directory of the current
process to the `newroot`. This is done using the `nix::unistd::chroot` function
from the `nix` crate.

When Trident is running in the `newroot`, it will have access to the file
systems of the target OS, but it will not have access to the file systems of the
servicing OS.

Trident will also mount certain directories from the servicing OS into the
`newroot` to ensure that necessary files and directories are available in the
context of the target OS. These directories include `/proc`, `/sys`, `/dev`,
and `/run`.

This is particularly relevant for any
[`postConfigure`](./Script-Hooks.md#post-configure-scripts)
scripts defined in the Host Configuration. These scripts are run from within
the chroot of the target OS, with the `$TARGET_ROOT` variable set to '/'.

Once Trident has completed its tasks in the context of the target OS, it will
exit the chroot and unmount the `newroot` and any bind mounts that were created.
This cleanup is done using the `umount` command.
