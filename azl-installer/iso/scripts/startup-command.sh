#!/bin/bash
INSTALLATION="/root/installation.sh"
chmod +x "$INSTALLATION"

chsh -s "$INSTALLATION" root
