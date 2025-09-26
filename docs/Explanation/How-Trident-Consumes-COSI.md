# How Trident Consumes COSI

Trident uses a Composable Operating System Image ([COSI](../Reference/COSI.md))
file to provision the contents of file systems that are not being newly created
or adopted. (Note that the
[`source`](../Reference/Host-Configuration/API-Reference/FileSystemSource.md) of
the file system can be specified in the Host Configuration.) A URL to this COSI
file as well as a hash of the COSI file's `metadata.json` file should be placed
in the [`image`
section](../Reference/Host-Configuration/API-Reference/OsImage.md) of the Host
Configuration.

This document explains the process Trident follows to read the COSI metadata and
stream the partition images to disk. For complete information on the structure
of a COSI file, please see the [COSI Reference Guide](../Reference/COSI.md).

## Reading the COSI Metadata

At the beginning of Trident's operations, Trident will load the COSI
`metadata.json` file into memory. At this point, Trident performs several checks
on the provided URL to ensure that Trident will be able to successfully read the
COSI file. These requirements differ by URL scheme:

- **HTTP/HTTPS (`http://`, `https://`)**: The web server hosting the images must
  support HTTP Range Requests. Trident will search for an `Accept-Ranges` header
  in the server's response.
- **OCI (`oci://`)**: The OCI registry must allow for anonymous image pulls. In
  addition, Trident expects that the referenced artifact contains exactly one
  layer.

At this point, Trident also calculates the hash of the COSI metadata. If a hash
was provided in the [`image`
section](../Reference/Host-Configuration/API-Reference/OsImage.md) of the Host
Configuration, the calculated hash is compared with the provided hash to verify
the integrity of the COSI file.

## Streaming and Verifying Images

A key feature of Trident's design is its ability to perform **sparse reads** of
the COSI file, which is critical for efficiency. Trident uses the COSI metadata
to determine the exact byte offset and size of each partition image in the COSI
file. Given this information, Trident performs one HTTP read request per
partition image. Importantly, this functionality requires that the hosting
server supports HTTP Range Requests. Using information from the Host
Configuration, Trident then writes the streamed partition image data directly to
the appropriate block device path on disk.

As Trident streams an image to its destination partition, it performs an
integrity check. The COSI metadata contains a SHA384 hash for each partition
image file. Trident calculates the hash of the image as it is being written and,
upon completion, verifies it against the hash provided in the metadata. This
ensures the integrity of the partition image.
