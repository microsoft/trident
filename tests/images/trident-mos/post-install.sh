# Use more intuitive path for the ISO mount
ln -s /run/initramfs/live /trident_cdrom

# Load the config from the CDROM so that the user can patch it
if [ ! -d /etc/trident ]; then
    mkdir /etc/trident
fi
ln -s /trident_cdrom/trident-config.yaml /etc/trident/config.yaml

# Compile and load Trident SELinux module (this is otherwise handled in trident.spec)
cd /usr/share/selinux/packages/trident
make -f /usr/share/selinux/devel/Makefile trident.pp
semodule -i trident.pp