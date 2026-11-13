# Add short alias to see trident's logs
alias tridentlog="journalctl -u trident -u trident-network"

# Open journalctl immediately after autologin
journalctl -f --no-tail -u trident -u trident-network