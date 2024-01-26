# Host Configuration Validation

Trident supports the `validate` subcommand to validate a Host Configuration YAML
file.

- [Host Configuration Validation](#host-configuration-validation)
  - [Validate a Host Configuration YAML File](#validate-a-host-configuration-yaml-file)
  - [Validate Host Configuration Inside a Trident Configuration YAML File](#validate-host-configuration-inside-a-trident-configuration-yaml-file)
  - [Online Validation](#online-validation)
  - [Expected Output](#expected-output)

## Validate a Host Configuration YAML File

To validate a Host Configuration YAML file offline (eg. in your dev box), use
the following command:

```bash
trident validate --host-config /path/to/host-config.yaml
```

## Validate Host Configuration Inside a Trident Configuration YAML File

To validate a Host Configuration inside a Trident configuration YAML, use the
following command:

```bash
trident validate --config /path/to/trident-config.yaml
```

*NOTE: The Host Configuration must be embedded inside the Trident configuration,
or be a file that can be accessed locally.*

## Online Validation

To validate a Host Configuration YAML file online (eg. in your target host), you
can use a bare invocation to read the default Trident Configuration file from
the default location:

```bash
trident validate
```

Trident will read the embedded Host Configuration or read the file being pointed
to, and validate it.

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
