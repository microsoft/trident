# Use more intuitive path for the ISO mount
ln -s /run/initramfs/live /trident_cdrom

# Load the config from the CDROM so that the user can patch it
if [ ! -d /etc/trident ]; then
    mkdir -p /etc/trident
fi
ln -s /trident_cdrom/trident-config.yaml /etc/trident/config.yaml

# Ensure /etc/rcp-agent/ exists
if [ ! -d /etc/rcp-agent ]; then
    mkdir -p /etc/rcp-agent
fi

# Link rcp-agent config from the ISO
ln -s /trident_cdrom/rcp-agent.toml /etc/rcp-agent/config.toml

# Compile and load Trident SELinux module (this is otherwise handled in trident.spec)
cd /usr/share/selinux/packages/trident
make -f /usr/share/selinux/devel/Makefile trident.pp
semodule -i trident.pp