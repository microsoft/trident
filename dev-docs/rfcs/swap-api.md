# Swap API Proposal

- [Swap API Proposal](#swap-api-proposal)
  - [Background](#background)
  - [Thesis and Proposal](#thesis-and-proposal)
    - [Alternatives](#alternatives)
  - [Supporting Arguments](#supporting-arguments)
  - [Rationale](#rationale)
  - [Prism Compatibility](#prism-compatibility)
  - [On GA Support of Swap Partitions](#on-ga-support-of-swap-partitions)
  - [On Swap Files](#on-swap-files)

## Background

Currently in trident, to create a swap partition, we do it through the
filesystem API:

```yaml
storage:
  # ... disk & partition declaration
  filesystems:
    deviceId: myPartition
    source: new
    type: swap
```

This means we consider swap as a filesystem type and adds a special case to the
filesystem struct to handle it. Swap is currently the only filesystem 'type'
that cannot be mounted.

## Thesis and Proposal

I want to argue that swap is NOT a filesystem and we should NOT be treating it
as such. Instead, I propose the following API which aligns well with the
existing Host Config / Prism model. This format also allows for forward
compatibility since we could eventually add more parameters to the swap section.
(e.g priority, discard policy, etc)

```yaml
storage:
  # ... disk & partition declaration
  swap:
    # Long form for forward compatibility  
    - deviceId: myPartition
    # Or the short-hand:
    - myPartition        
```

### Alternatives

- Semi-implicit Declaration Through Partition Type

  ```yaml
  storage:
    disk:
      # disk data ...
      partititons:
        - id: mySwapPart
          size: 1G
          type: swap    # <-- this right here
  ```

  > [Comment from Chris Gunn]
  >
  > There is some precedent for this solution with the ESP partition. Within
  > Prism, setting a partition to the 'esp' type carries a bunch of implications
  > beyond just setting the UUID in the partition table.
  >
  > On the flip side, I think it is also valid to liken swap to RAID, verity, etc.
  > That is, we split the partition provisioning from how it is used.

## Supporting Arguments

- Swap is strictly NOT a filesystem. Swap is a really a kernel feature for
  extending virtual memory.
- `swap` is not a recognized filesystem type in the kernel.
- Swap is not handled via the same tools as filesystems: `mkfs`, `mount`, etc.
  Instead it requires `mkswap`, `swapon`/`swapoff`.
- Swap is not mounted in any traditional way, it is treated as an entirely
  different concept by the kernel. Swap is not exposed in `/proc/mounts`, but
  rather in its own `/proc/swap`.

## Rationale

- Stop treating SWAP as a FS type externally and internally per the thesis of
  this document.
- Generally trident doesn't deal with swap partitions for anything other than
  creating them.
- Internally, encoding swap vs real fs is annoying.
- The API is much shorter.
- We can treat swap as its own graph entity, which give us more control in how
  it's used.
- Less special cases of the filesystem struct. Swap is currently the only 'type'
  that CANNOT be mounted. Less validation!

## Prism Compatibility

- Prism does not currently support creating swap. Prism could eventually adopt
this API too.
- Prism does NOT accept swap as a filesystem type. ([filesystem type | Azure
Linux Image
Tools](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/api/configuration/filesystem.html#type-string))
- Prism only recognizes swap as such in partition types. (Similar to alternative
  no. 1) But this only performs the GPT tagging and DOES NOT CREATE THE SWAP AREA.

## On GA Support of Swap Partitions

As of this writing, there is no definitive decision on whether we will support
swap partitions in GA. If we do, we will do so with this API. The immediate goal
of this RFC is to remove the special case of swap in the filesystem struct, not
to make a statement regarding GA support of swap partitions.

## On Swap Files

While strictly out of the scope of this RFC, it is worth mentioning that swap
files are a thing we may want to eventually support, so API compatibility must
be considered.

An eventual API for swap files would need to go through its own review process,
but I believe swap files would more likely be a property of the `os` section,
rather than the storage section, as they really are primarily an OS concept and
they don't directly relate to storage configuration. For example:

```yaml
os:
  swapFiles:
    - path: /path/to/swapfile
      size: 1G
```

In this case, there is a clear separation between swap partitions and swap files
in the API.
