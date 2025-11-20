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

**Note**: If you encounter an "access denied" error, SELinux may be preventing Trident from writing to that location. Use `/tmp` as shown above.

## Extract and Review the Bundle

To examine the bundle contents:

```bash
tar --use-compress-program=unzstd -xvf trident-diagnostics.tar.zst
cd trident-diagnostics
```

The bundle contains:
- `report.json` - System metadata and diagnostics summary
- `datastore.sqlite` - Trident datastore (and variants if present)
- `logs/trident-full.log` - Current execution log
- `logs/trident-metrics.jsonl` - Current metrics
- `logs/historical/` - Logs from past servicing operations

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
