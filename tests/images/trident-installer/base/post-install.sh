# Add the necessary directories for the audit logs so that auditd can start
mkdir -p /var/log/audit
# Use more intuitive path for the ISO mount
ln -s -T /run/initramfs/live /trident_cdrom

# Load the config from the CDROM so that the user can patch it
if [ ! -d /etc/trident ]; then
    mkdir /etc/trident
fi
ln -s -T /trident_cdrom/trident-config.yaml /etc/trident/config.yaml

if [ ! -d /etc/systemd/system/trident-install.service.d ]; then
    mkdir /etc/systemd/system/trident-install.service.d
fi
ln -s -T /trident_cdrom/trident-override.conf /etc/systemd/system/trident-install.service.d/override.conf