# API Changes Related to COSI

1. [`verityFilesystems` to be deprecated and replaced by
   `verity`](#verityfilesystems-to-be-deprecated-and-replaced-by-verity).
2. [Remove `cosi` internal parameter](#remove-cosi-internal-parameter).
3. [Changes to `source` and `type` in `filesystems`
   section](#changes-to-source-and-type-in-filesystems-section).
4. [Changes to `osImage` section](#changes-to-osimage-section).

## `verityFilesystems` to be deprecated and replaced by `verity`

Currently, both `verityFilesystems` and `verity` coexist in Trident. We are introducing `verity`
gradually and will drop support for `verityFilesystems` when we drop support for partition images.

`verityFilesystems`:

```yaml
verityFilesystems:
  - name: root
    dataDeviceId: root-data
    hashDeviceId: root-hash
    dataImage:
      url: http://NETLAUNCH_HOST_ADDRESS/files/verity_root.rawzst
      sha256: ignored
      format: raw-zst
    hashImage:
      url: http://NETLAUNCH_HOST_ADDRESS/files/verity_roothash.rawzst
      sha256: ignored
      format: raw-zst
    type: ext4
    mountPoint:
      path: /
      options: defaults,ro
```

`verity` equivalent:

```yaml
verity:
  - id: root
    name: root
    dataDeviceId: root-data
    hashDeviceId: root-hash
filesystems:
  - deviceId: root
    type: ext4
    source:
      type: os-image
```
*Note that the URL for all images is specified in the [`image`](#changes-to-osimage-section)
section.

## Remove `cosi` internal parameter

Currently Trident has an internal parameter which signals our use of COSI in the Host Configuration.
Once OS images become the default, this parameter will not be necessary.

## Changes to `source` and `type` in `filesystems` section

1. Change the enum variants for `source`, by removing `Image()` and `EspImage()` variants
   (corresponding to partition images), renaming `OsImage` to `Image`, and renaming `Create` to
   `New`.

2. Set the default to be `Image`. This way, `source` does not need to be specified if the filesystem
   is derived from an OS image, which we expect will be true for most filesystems.

    Previously:

    ```rust
    enum Source {
      #[default]
      Create,
      Image(Image),
      EspImage(Image),
      OsImage,
      Adopted,
    }
    ```

    New, finalized:

    ```rust
    enum Source {
      #[default]
      Image,
      New,
      Adopted,
    }
    ```

3. Filesystem `type` cannot be specified for filesystems sourced from an `Image` (OS image). `type`
   will be an optional field for `New` and `Adopted`. For `New` the default will be `type: ext4`.
   For `Adopted` the default will be `type: auto`.

### Example

Previous configuration:

```yaml
filesystems:
  # Sourced from an OS image
  - deviceId: esp
    type: vfat
    source:
      type: os-image
    mountPoint:
      path: /boot/efi
      options: umask-0077

  # Create a new filesystem
  - deviceId: trident
    type: ext4
    mountPoint: /var/lib/trident

  # Adopt an existing filesystem
  - deviceId: srv
    source:
      type: adopted
    type: auto
    mountPoint: /srv
```

New configuration:

```yaml
filesystems:
  # Sourced from an OS image
  - deviceId: esp
    mountPoint:
      path: /boot/efi
      options: umask=0077
  
  # Create a new ext4 filesystem
  - deviceId: trident
    source: new
    mountPoint: /var/lib/trident

  # Adopt an existing filesystem
  - deviceId: srv
    source: adopted
    mountPoint: /srv
```

### Discussion Points

1. Naming of source `type`. Currently we have `type` and `fsType`. Some alternatives to source
   `type` (which can be `create`, `os-image`, and `adopted`) include: `mode`, `sourceKind`, and
   `kind`.

    **Resolution: keep `type` and `source` for consistency and ease of migration process.**

2. Naming of filesystem type inside `source`. Currently the field is named `fsType` since `type` in
   this context refers to the source type.
      * **[Final Choice]** Alternative 1: Do not nest the filesystem type under source, as such:
  
        ```yaml
        filesystems:
          - deviceId: trident
            source: create
            type: ext4
        ```

        `type` can be made an optional field that defaults to `ext4` if `source: create` and `auto`
        if `source: adopted`.

      * Alternative 2: Keep nesting, but set `type: ext4` as default so that `source: create` can be
        used as a shorthand. The following would be equivalent to the code block above:
  
        ```yaml
        filesystems:
          - deviceId: trident
            source: create
        ```

        For all other types of file system we would revert to nesting:

        ```yaml
        filesystems:
          - deviceId: trident
            source:
              type: create
              fsType: vfat
        ```

      * Alternative 3: Condense into a singular map

        ```yaml
        filesystems:
          # Sourced from an OS image
          - deviceId: root
            mountPoint: /

          # Create a new ext4
          - deviceId: data
            create: ext4
            mountPoint: /data

          # Adopt an existing filesystem
          - deviceId: srv
            adopted: auto
            mountPoint: /srv
        ```

        OS image can either function as the "none" value, or it can be a default variant that does
        not need to be specified but can be.

    **Resolution: Alternative 1**

## Changes to `osImage` section

1. Rename the section to be `image`.
2. Remove `type` field. Currently, this is always set to `type: cosi`. Since there is only one type
   of OS image that Trident currently supports, we will remove this field. Instead, we can infer the
   file type from the file contents.
3. Add field `sha384` which will accept a Sha384 hash of the entire OS image.

### Example

Previous configuration:

```yaml
osImage:
  type: cosi
  url: http://NETLAUNCH_HOST_ADDRESS/files/regular.cosi
```

New configuration:

```yaml
image:
  url: http://NETLAUNCH_HOST_ADDRESS/files/regular.cosi
  sha384: e8e4d9727695438c7f5c91347e50e3d68... (not a real hash)
```
