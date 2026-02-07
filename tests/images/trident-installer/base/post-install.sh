# Add the necessary directories for the audit logs so that auditd can start
mkdir -p /var/log/audit
# Use more intuitive path for the ISO mount
ln -s -T /run/initramfs/live /trident_cdrom

# Compile and load Trident SELinux module (this is otherwise handled in trident.spec)
cd /usr/share/selinux/packages/trident
make -f /usr/share/selinux/devel/Makefile trident.pp
semodule -i trident.pp

# Allow various purpose installer images to use post-install.sh, including
# those that do not use Trident config for installation ('no-trident-config').
if [ "$1" != "no-trident-config" ]; then
    # Load the config from the CDROM so that the user can patch it
    if [ ! -d /etc/trident ]; then
        mkdir /etc/trident
    fi
    ln -s -T /trident_cdrom/trident-config.yaml /etc/trident/config.yaml

    if [ ! -d /etc/systemd/system/trident-install.service.d ]; then
        mkdir /etc/systemd/system/trident-install.service.d
    fi
    ln -s -T /trident_cdrom/trident-override.conf /etc/systemd/system/trident-install.service.d/override.conf
fi