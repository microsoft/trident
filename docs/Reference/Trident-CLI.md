# Trident Command Line Interface Documentation

Usage:

```
trident [OPTIONS] <COMMAND>
```

Argument summary:

```
Commands:
  install             Initiate an install of Azure Linux
  update              Start or continue an A/B update from an
                          existing install
  commit              Detect whether an install or update
                          succeeded, and update the boot order
                          accordingly
  rebuild-raid        Rebuild software RAID arrays managed by
                          Trident
  get                 Query the current state of the system
  validate            Validate the provided Host Configuration
  offline-initialize  Initialize for a system that wasn't
                          installed by Trident
  help                Print this message or the help of the given
                          subcommand(s)

Options:
  -v, --verbosity <VERBOSITY>
          Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
          [default: DEBUG]
  -V, --version
          Print version
```


# Subcommands

- [install](#install)
- [update](#update)
- [commit](#commit)
- [listen](#listen)
- [rebuild-raid](#rebuild-raid)
- [start-network](#start-network)
- [get](#get)
- [validate](#validate)
- [offline-initialize](#offline-initialize)
- [help](#help)


## install

Initiate an install of Azure Linux

Usage:

```
trident install [OPTIONS] [CONFIG]
```

Argument summary:

```
Arguments:
  [CONFIG]  The new configuration to apply [default:
            /etc/trident/config.yaml]

Options:
      --allowed-operations [<ALLOWED_OPERATIONS>...]
          Comma-separated list of operations that Trident will be
          allowed to perform [default: stage,finalize] [possible
          values: stage, finalize]
  -v, --verbosity <VERBOSITY>
          Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
          [default: DEBUG]
  -s, --status <STATUS>
          Path to save the resulting Host Status
  -e, --error <ERROR>
          Path to save an eventual fatal error
      --multiboot
          Allow Trident to perform a multiboot install
```


### Argument Details

#### <span style="color:#d96125;font-family:monospace;">&lt;CONFIG&gt;</span>

The new configuration to apply

Default: `/etc/trident/config.yaml`


#### <span style="color:#d96125;font-family:monospace;">--allowed_operations &lt;ALLOWED_OPERATIONS&gt;</span>

Comma-separated list of operations that Trident will be allowed to perform

Possible values:

- `stage`
- `finalize`

Default: `stage,finalize`


#### <span style="color:#d96125;font-family:monospace;">--status &lt;STATUS&gt;</span>

Path to save the resulting Host Status


#### <span style="color:#d96125;font-family:monospace;">--error &lt;ERROR&gt;</span>

Path to save an eventual fatal error


#### <span style="color:#d96125;font-family:monospace;">--multiboot &lt;MULTIBOOT&gt;</span>

Allow Trident to perform a multiboot install


#### <span style="color:#d96125;font-family:monospace;">--verbosity &lt;VERBOSITY&gt;</span>

Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]

Default: `DEBUG`


## update

Start or continue an A/B update from an existing install

Usage:

```
trident update [OPTIONS] [CONFIG]
```

Argument summary:

```
Arguments:
  [CONFIG]  The new configuration to apply [default:
            /etc/trident/config.yaml]

Options:
      --allowed-operations [<ALLOWED_OPERATIONS>...]
          Comma-separated list of operations that Trident will be
          allowed to perform [default: stage,finalize] [possible
          values: stage, finalize]
  -v, --verbosity <VERBOSITY>
          Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
          [default: DEBUG]
  -s, --status <STATUS>
          Path to save the resulting Host Status
  -e, --error <ERROR>
          Path to save an eventual fatal error
```


### Argument Details

#### <span style="color:#d96125;font-family:monospace;">&lt;CONFIG&gt;</span>

The new configuration to apply

Default: `/etc/trident/config.yaml`


#### <span style="color:#d96125;font-family:monospace;">--allowed_operations &lt;ALLOWED_OPERATIONS&gt;</span>

Comma-separated list of operations that Trident will be allowed to perform

Possible values:

- `stage`
- `finalize`

Default: `stage,finalize`


#### <span style="color:#d96125;font-family:monospace;">--status &lt;STATUS&gt;</span>

Path to save the resulting Host Status


#### <span style="color:#d96125;font-family:monospace;">--error &lt;ERROR&gt;</span>

Path to save an eventual fatal error


#### <span style="color:#d96125;font-family:monospace;">--verbosity &lt;VERBOSITY&gt;</span>

Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]

Default: `DEBUG`


## commit

Detect whether an install or update succeeded, and update the boot order accordingly

Usage:

```
trident commit [OPTIONS]
```

Argument summary:

```
Options:
  -s, --status <STATUS>
          Path to save the resulting Host Status
  -v, --verbosity <VERBOSITY>
          Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
          [default: DEBUG]
  -e, --error <ERROR>
          Path to save an eventual fatal error
```


### Argument Details

#### <span style="color:#d96125;font-family:monospace;">--status &lt;STATUS&gt;</span>

Path to save the resulting Host Status


#### <span style="color:#d96125;font-family:monospace;">--error &lt;ERROR&gt;</span>

Path to save an eventual fatal error


#### <span style="color:#d96125;font-family:monospace;">--verbosity &lt;VERBOSITY&gt;</span>

Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]

Default: `DEBUG`


## listen

Usage:

```
trident listen [OPTIONS]
```

Argument summary:

```
Options:
  -s, --status <STATUS>
          Path to save the resulting Host Status
  -v, --verbosity <VERBOSITY>
          Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
          [default: DEBUG]
  -e, --error <ERROR>
          Path to save an eventual fatal error
```


### Argument Details

#### <span style="color:#d96125;font-family:monospace;">--status &lt;STATUS&gt;</span>

Path to save the resulting Host Status


#### <span style="color:#d96125;font-family:monospace;">--error &lt;ERROR&gt;</span>

Path to save an eventual fatal error


#### <span style="color:#d96125;font-family:monospace;">--verbosity &lt;VERBOSITY&gt;</span>

Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]

Default: `DEBUG`


## rebuild-raid

Rebuild software RAID arrays managed by Trident

Usage:

```
trident rebuild-raid [OPTIONS]
```

Argument summary:

```
Options:
  -c, --config <CONFIG>
          The new configuration to work from
  -v, --verbosity <VERBOSITY>
          Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
          [default: DEBUG]
  -s, --status <STATUS>
          Path to save the resulting HostStatus
  -e, --error <ERROR>
          Path to save an eventual fatal error
```


### Argument Details

#### <span style="color:#d96125;font-family:monospace;">--config &lt;CONFIG&gt;</span>

The new configuration to work from


#### <span style="color:#d96125;font-family:monospace;">--status &lt;STATUS&gt;</span>

Path to save the resulting HostStatus


#### <span style="color:#d96125;font-family:monospace;">--error &lt;ERROR&gt;</span>

Path to save an eventual fatal error


#### <span style="color:#d96125;font-family:monospace;">--verbosity &lt;VERBOSITY&gt;</span>

Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]

Default: `DEBUG`


## start-network

Configure OS networking based on Trident Configuration

Usage:

```
trident start-network [OPTIONS] [CONFIG]
```

Argument summary:

```
Arguments:
  [CONFIG]  The new configuration to apply [default:
            /etc/trident/config.yaml]

Options:
  -v, --verbosity <VERBOSITY>
          Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
          [default: DEBUG]
```


### Argument Details

#### <span style="color:#d96125;font-family:monospace;">&lt;CONFIG&gt;</span>

The new configuration to apply

Default: `/etc/trident/config.yaml`


#### <span style="color:#d96125;font-family:monospace;">--verbosity &lt;VERBOSITY&gt;</span>

Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]

Default: `DEBUG`


## get

Query the current state of the system

Usage:

```
trident get [OPTIONS] [KIND]
```

Argument summary:

```
Arguments:
  [KIND]  What data to retrieve [default: status] [possible values:
          configuration, status, last-error]

Options:
  -o, --outfile <OUTFILE>
          Path to save the resulting output
  -v, --verbosity <VERBOSITY>
          Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
          [default: DEBUG]
```


### Argument Details

#### <span style="color:#d96125;font-family:monospace;">&lt;KIND&gt;</span>

What data to retrieve

Possible values:

- `configuration`
- `status`
- `last-error`

Default: `status`


#### <span style="color:#d96125;font-family:monospace;">--outfile &lt;OUTFILE&gt;</span>

Path to save the resulting output


#### <span style="color:#d96125;font-family:monospace;">--verbosity &lt;VERBOSITY&gt;</span>

Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]

Default: `DEBUG`


## validate

Validate the provided Host Configuration

Usage:

```
trident validate [OPTIONS] [CONFIG]
```

Argument summary:

```
Arguments:
  [CONFIG]  Path to a Host Configuration file [default:
            /etc/trident/config.yaml]

Options:
  -v, --verbosity <VERBOSITY>
          Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
          [default: DEBUG]
```


### Argument Details

#### <span style="color:#d96125;font-family:monospace;">&lt;CONFIG&gt;</span>

Path to a Host Configuration file

Default: `/etc/trident/config.yaml`


#### <span style="color:#d96125;font-family:monospace;">--verbosity &lt;VERBOSITY&gt;</span>

Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]

Default: `DEBUG`


## offline-initialize

Initialize for a system that wasn't installed by Trident

Usage:

```
trident offline-initialize [OPTIONS] [HS_PATH]
```

Argument summary:

```
Arguments:
  [HS_PATH]  Path to a Host Status file (deprecated)

Options:
      --lazy-partitions [<LAZY_PARTITIONS>...]
          Provide lazy partition information overrides for `-b`
          partitions
  -v, --verbosity <VERBOSITY>
          Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
          [default: DEBUG]
      --disk <DISK>
          Provide disk path [default: /dev/sda]
```


### Argument Details

#### <span style="color:#d96125;font-family:monospace;">&lt;HS_PATH&gt;</span>

Path to a Host Status file (deprecated)

If not provided, Trident will infer one based on the state of the system and history information left by Image Customizer.

Conflicts with:

- `--lazy_partitions <LAZY_PARTITIONS>`


#### <span style="color:#d96125;font-family:monospace;">--lazy_partitions &lt;LAZY_PARTITIONS&gt;</span>

Provide lazy partition information overrides for `-b` partitions

This is a comma-separated list of `<b-partition-name>`:`<b-partition-partuuid>` pairs.

Conflicts with:

- `<HS_PATH>`


#### <span style="color:#d96125;font-family:monospace;">--disk &lt;DISK&gt;</span>

Provide disk path

Default: `/dev/sda`

Conflicts with:

- `<HS_PATH>`


#### <span style="color:#d96125;font-family:monospace;">--verbosity &lt;VERBOSITY&gt;</span>

Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]

Default: `DEBUG`


## help

Print this message or the help of the given subcommand(s)

Usage:

```
trident help [COMMAND]
```

Argument summary:

```
Commands:
  install             Initiate an install of Azure Linux
  update              Start or continue an A/B update from an
                          existing install
  commit              Detect whether an install or update
                          succeeded, and update the boot order
                          accordingly
  rebuild-raid        Rebuild software RAID arrays managed by
                          Trident
  get                 Query the current state of the system
  validate            Validate the provided Host Configuration
  offline-initialize  Initialize for a system that wasn't
                          installed by Trident
  help                Print this message or the help of the given
                          subcommand(s)
```


