#!/bin/bash

set -eux

mount -t overlay overlay -o lowerdir=/etc,upperdir=/var/lib/trident-overlay/etc-rw/upper,workdir=/var/lib/trident-overlay/etc-rw/work /etc
# Workaround for https://dev.azure.com/mariner-org/ECF/_workitems/edit/7349/
chmod o+rx /etc