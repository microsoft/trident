# Use more intuitive path for the ISO mount
ln -s /run/initramfs/live /trident_cdrom

# Load the config from the CDROM so that the user can patch it
ln -s /trident_cdrom/trident-config.yaml /etc/trident/config.yaml