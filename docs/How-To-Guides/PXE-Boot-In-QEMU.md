# Running the Provisioning OS via PXE

1. First, you need to get five files. These are distributed together as `pxe-artifacts.zip`:

   * **bootx64.efi** This is the shim binary that is hard-coded to launch _grubx64.efi_.
   * **grubx64.efi** This is a copy of the "noprefix" version of GRUB2.
   * **grub.cfg** Configuration file for GRUB that directs it to load the following files.
   * **vmlinuz** The Linux kernel binary.
   * **initrd.img** An initrd containing Trident and a placeholder Trident configuration.

2. Move the first four files to a `tftp` directory to serve them:

   ```bash
   mkdir tftp
   cp bootx64.efi grubx64.efi grub.cfg vmlinuz tftp/
   ```

3. Using _patch.py_ from the [argus-toolkit
   repository](https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit), insert your own
   _trident.yaml_ into the initrd. You can read more about this YAML configuration in the main
   README.

   ```bash
   python3 initrd.img tftp/initird.img trident.yaml
   ```

4. Create `disk.img` to use as an emulated hard drive:

   ```bash
   truncate -s 20G disk.img
   ```

5. Run TPM emulator (the `swtpm` call needs to be rerun each time you launch QEMU).

   ```bash
   mkdir /tmp/mytpm1
   swtpm socket \
       --tpmstate dir=/tmp/mytpm1 \
       --ctrl type=unixio,path=/tmp/mytpm1/swtpm-sock \
       --log level=20
    ```

6. While `swtpm` is still running, launch QEMU in a second terminal.

   ```bash
   qemu-system-x86_64 -machine q35 -cpu host -smp 2 -m 4G -accel kvm -serial stdio \
       -netdev user,id=net0,tftp=./tftp,bootfile=/bootx64.efi \
       -device virtio-net-pci,netdev=net0 \
       -drive if=pflash,format=raw,file=/usr/share/OVMF/OVMF_CODE_4M.fd,readonly=on \
       -chardev socket,id=chrtpm,path=/tmp/mytpm1/swtpm-sock \
       -tpmdev emulator,id=tpm0,chardev=chrtpm \
       -device tpm-tis,tpmdev=tpm0 \
       -drive format=raw,file=disk.raw
    ```
