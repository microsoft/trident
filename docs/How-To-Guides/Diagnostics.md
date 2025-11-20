# Generate Diagnostics and Support Bundles

Trident provides a diagnose command that collects system information, logs, and configuration data into a single support bundle. This is useful for troubleshooting issues and reporting bugs.

## What Information is Collected?

The diagnostics bundle includes:

- **System Information**: Host platform details, virtualization info, disk configuration
- **Trident Version**: Version information of the running Trident instance
- **Host Status**: Current state from the Trident datastore
- **Current Logs**: Active Trident execution log and metrics (see [View Trident's Background Log](./View-Trident's-Background-Log.md) for more details)
- **Historical Logs**: Persisted logs and metrics from past servicing operations
- **Datastore Files**: Current, temporary, and configured datastore databases

All collected data is packaged into a compressed tarball (`.tar.zst`).

## Generating a Diagnostics Bundle

To generate a diagnostics bundle, use the `diagnose` command:

```bash
sudo trident diagnose --output /tmp/trident-diagnostics.tar.zst
```

This will create a compressed bundle at `/tmp/trident-diagnostics.tar.zst` containing all diagnostic information.

### Required Arguments

- `--output` or `-o`: Path where the support bundle will be saved

### Optional Arguments

- `--verbosity` or `-v`: Logging verbosity level (default: `DEBUG`)
  - Available levels: `OFF`, `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE`

**Note**: If you encounter an "access denied" error when running this command, it may be due to SELinux configuration preventing Trident from writing to the specified location. In this case, try outputting the bundle to `/tmp` as shown in the example above.

## Bundle Structure

The generated bundle has the following structure:

```
trident-diagnostics/
├── report.json                    # Diagnostic report with metadata
└── logs/
    ├── trident-full.log           # Current Trident execution log
    ├── trident-metrics.jsonl      # Current Trident metrics
    ├── historical/                # Logs from past servicing
    │   ├── trident-<servicing_state>-<timestamp>.log
    │   ├── trident-metrics-<servicing_state>-<timestamp>.log
    │   └── ...
    ├── datastore.sqlite           # Default datastore
    ├── datastore-tmp.sqlite       # Temporary datastore (if applicable)
    └── datastore-configured.sqlite # Configured datastore (if applicable)
```

## Understanding the Report

The `report.json` file contains structured information about your system:

### Report Fields

| Field               | Description                                              |
| ------------------- | -------------------------------------------------------- |
| `timestamp`         | When the report was generated (RFC3339 format)           |
| `version`           | Trident version                                          |
| `host_description`  | Platform, virtualization, container, and disk information|
| `host_status`       | Current state from the datastore (if available)          |
| `collected_files`   | List of files included in the bundle with metadata      |

### Host Description

The host description includes:

- `is_container`: Whether Trident is running in a container
- `is_virtual`: Whether the host is a virtual machine
- `virt_type`: Type of virtualization (e.g., `qemu`, `hyperv`, `none detected`)
- `platform_info`: Detailed platform metadata
- `disk_info`: Block device information from `lsblk`

## Use Cases

### Bug Reports

When reporting a bug, generate a diagnostics bundle and attach it to your issue:

```bash
sudo trident diagnose --output /tmp/trident-bug-report.tar.zst
```

The bundle provides developers with context about your system state.

### Troubleshooting Failed Updates

If an update fails, the bundle includes both current and historical logs showing what happened during each stage:

```bash
sudo trident diagnose --output /tmp/failed-update-diagnostics.tar.zst
```

## Extracting the Bundle

To extract and examine the contents of a diagnostics bundle:

```bash
tar --use-compress-program=unzstd -xvf trident-diagnostics.tar.zst
```

This will extract the contents to a `trident-diagnostics/` directory in your current location.

## Privacy and Security Considerations

The diagnostics bundle may contain sensitive information:

- System configuration details
- Disk layout and partition information
- Datastore contents (which may include custom configurations)
- Execution logs (which may contain system paths and information)

**Review the bundle contents before sharing** and ensure it's appropriate for your security and privacy requirements.

## Related Documentation

- [View Trident's Background Log](./View-Trident's-Background-Log.md) - For more details about log files
- [Trident CLI Reference](../Reference/Trident-CLI.md#diagnose) - Complete CLI documentation
