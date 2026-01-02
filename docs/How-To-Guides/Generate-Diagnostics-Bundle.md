# Generate a Diagnostics Bundle

When troubleshooting Trident issues or reporting bugs, you can collect system information, logs, and configuration into a support bundle for analysis.

## Generate the Bundle

Run the diagnose command to create a support bundle:

```bash
sudo trident diagnose --output /tmp/trident-diagnostics.tar.zst
```

This creates a compressed bundle containing:
- System and platform information
- Trident version and configuration
- Current and historical logs (see [View Trident's Background Log](./View-Trident's-Background-Log.md))
- Datastore files

### Optional Flags

For more comprehensive diagnostics, you can include additional information:

- `--journal`: Includes the complete system journal from the current boot, including kernel messages (`dmesg`). Useful for diagnosing system-level issues.
- `--selinux`: Includes the SELinux audit log (`/var/log/audit/audit.log`). Useful for diagnosing SELinux policy denials.

**Note**: If you encounter an "access denied" error, SELinux may be preventing Trident from writing to that location. Use `/tmp` as shown above.

## Extract and Review the Bundle

To examine the bundle contents:

```bash
tar --use-compress-program=unzstd -xvf trident-diagnostics.tar.zst
cd trident-diagnostics
```

The bundle contains:

**Report file:**
- `report.json` - Comprehensive diagnostics report including:
  - Timestamp and Trident version
  - Host description (container/VM detection, virtualization type, platform info)
  - Block device and mount information
  - Health check status (health check systemd services)
  - TPM 2.0 pcrlock log
  - Trident service status and journal
  - Host status
  - Collection failures (see below)

**Log files:**
- `logs/trident-full.log` - Current execution log
- `logs/trident-metrics.jsonl` - Current metrics
- `logs/historical/` - Logs and metrics from past servicing operations

**Datastore files:**
- `datastore.sqlite` - Default Trident datastore
- `datastore-tmp.sqlite` - Temporary datastore (if present)
- `datastore-configured.sqlite` - Configured datastore (if present)

**System configuration:**
- `files/fstab` - File system mount configuration (`/etc/fstab`)
- `tpm/pcrlock.json` - TPM 2.0 pcrlock policy (if available)

**Optional files (when flags are specified):**
- `full-journal` - Full system journal (with `--journal`)
- `selinux/audit.log` - SELinux audit log (with `--selinux`)

## Collection Failures

The `report.json` includes a `collection_failures` list of items that could not be collected. Some failures are expected depending on system configuration (e.g., no pcrlock on systems without TPM 2.0).

## Share the Bundle for Support

When reporting an issue:

1. Generate the bundle:
   ```bash
   sudo trident diagnose --output /tmp/trident-issue-12345.tar.zst
   ```

2. Review the contents to ensure no sensitive data is included

3. Attach the bundle to your bug report or support request

**Privacy Note**: The bundle contains system configuration, disk layout, and execution logs. Review before sharing externally.

## Related Documentation

- [View Trident's Background Log](./View-Trident's-Background-Log.md) - For more details about log files
- [Trident CLI Reference](../Reference/Trident-CLI.md#diagnose) - Complete CLI documentation
