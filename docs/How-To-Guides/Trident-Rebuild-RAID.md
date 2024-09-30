# Trident Rebuild RAID

Trident supports the `rebuild-raid` subcommand to rebuild RAID arrays. Refer to
[Rebuild RAID](/docs/Explanation/Rebuild-RAID.md) for a detailed explanation of
the term.

## Prerequisites

Before you start, make sure you've completed the following:

1. **Provisioned Host**: Ensure the host is provisioned with Trident.
2. **Disk Compatibility**: Replace the failed disk with a similar disk.
3. **Active RAID Members**: Verify an active copy of RAID members is available for recovery.
4. **Supported Configuration**: Ensure the replaced disk’s configuration contains recoverable RAID members or unformatted partitions.

## Steps

The aim here is to restore your RAID array to full functionality after a disk failure, using the Trident tool. This involves replacing the failed disk and ensuring the new disk is properly integrated into the RAID configuration.

### Step 1: Replace the Failed Disk

First, you need to physically replace the failed disk with a new one. Follow these detailed actions:

1. **Remove the Failed Disk**: Carefully extract the defective disk from the storage array.
2. **Install the New Disk**: Insert a new, compatible disk in its place. Make sure the new disk meets the required specifications of the failed disk to ensure compatibility and proper functionality.

### Step 2: Initiate the Rebuild RAID Operation with Trident

Once the new disk is installed, you’ll need to use Trident to initiate the rebuild process. Here’s how you do it:

1. **Run the Command**: Use the following command to start the rebuild process:
   
   ```bash
   trident rebuild-raid
   ```

2. **Monitor the Process**: Watch the rebuild process to ensure it completes successfully. 
 
   On successful validation, Trident will exit silently with a zero exit code.

   On validation failure, Trident will exit with a non-zero exit code and print
   the error that caused the rebuild process to fail.
   