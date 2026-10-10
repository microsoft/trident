#!/bin/bash
INSTALLATION="/root/installer/installation.sh"
chmod +x "$INSTALLATION"

chsh -s "$INSTALLATION" root
