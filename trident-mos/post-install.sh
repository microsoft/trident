# Use more intuitive path for the ISO mount
ln -s /run/initramfs/live /trident_cdrom

# Load the config from the CDROM so that the user can patch it
if [ ! -d /etc/trident ]; then
    mkdir /etc/trident
fi
ln -s /trident_cdrom/trident-config.yaml /etc/trident/config.yaml

# Enable trace logging for development
sed -i 's/--verbosity INFO/--verbosity DEBUG/' /usr/lib/systemd/system/trident.service