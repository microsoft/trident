# How Trident Consumes COSI

<!--
DELETE ME AFTER COMPLETING THE DOCUMENT!
---
Task: https://dev.azure.com/mariner-org/polar/_workitems/edit/13160
Title: How Trident Consumes COSI
Type: Explanation
Objective: Talk about how trident reads COSI files and its implications.
    eg. over http severs need to support range requests.
    Discus integrity validation. Link to cosi page in reference section.
-->

Trident uses a Composite OS Image ([COSI](../Reference/COSI.md)) file to
provision the contents of file systems that are not being newly created or
adopted. (Note that the
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
  support HTTP Range Requests. This is typically indicated by the
  `Accept-Ranges` header in the server's response and allows Trident to read
  each image file separately.
- **OCI (`oci://`)**: The OCI registry must allow for anonymous image pulls. In
  addition, Trident expects that the referenced artifact contains exactly one
  layer.

At this point, Trident also calculates the hash of the COSI metadata. If a hash
was provided in the [`image`
section](../Reference/Host-Configuration/API-Reference/OsImage.md) of the Host
Configuration, the calculated hash is compared with the provided hash to verify
the integrity of the COSI file.

## Streaming and Verifying Images

The COSI metadata contains information that allows Trident to seek the exact
location of each partition image in the COSI file. Paired witht the information
on from the Host Configuration, Trident is able to write each partition image
directly to the appropriate block device patch on disk. As Trident streams an
image to its destination partition, it performs a critical integrity check. The
COSI metadata contains a SHA384 hash for each image file. Trident calculates the
hash of the image as it is being written and, upon completion, verifies it
against the hash provided in the metadata. This ensures the integrity of the
partition image.
