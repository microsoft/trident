
# Two-Step Installation and Update

In addition to running `trident install` and `trident update` fully to
completion, Trident allows breaking these commands into separate stage and
finalize operations. The intent is that staging can happen without workload
disruption and finalizing can be delayed until the user is ready to reboot.

Staging an update involves streaming partition images and applying the indicated
OS and bootloader configuration. It generally takes on the order of 1-5 minutes,
but the workload can continue running while staging is in progress:

``` bash
trident update --allowed-operations stage /etc/trident/config.yaml
```

Finalizing an update just sets the new boot order and triggers a reboot:

``` bash
trident update --allowed-operations finalize /etc/trident/config.yaml
```
