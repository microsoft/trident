# Host Configuration Validation

Trident supports the `validate` subcommand to validate a Host Configuration YAML
file.

  - [Validate a Host Configuration YAML File](#validate-a-host-configuration-yaml-file)
  - [Online Validation](#online-validation)
  - [Expected Output](#expected-output)

## Validate a Host Configuration YAML File

To validate a Host Configuration YAML file offline (eg. in your dev box), use
the following command:

```bash
trident validate /path/to/host-config.yaml
```

## Online Validation

To validate a Host Configuration YAML file online (eg. in your target host), you
can use a bare invocation to read from `/etc/trident/config.yaml`, which is the
default Host Configuration path:

```bash
trident validate
```

## Expected Output

On successful validation, Trident will exit silently with a zero exit code.

On validation failure, Trident will exit with a non-zero exit code and print the
error that caused the validation to fail.

```text
[ERROR trident] Trident failed: Host config is invalid
    
    Caused by:
        0: The block device graph is invalid
        1: Block device 'some_raid' of kind 'raid-array' references non-existent block device 'does-not-exist'
```
