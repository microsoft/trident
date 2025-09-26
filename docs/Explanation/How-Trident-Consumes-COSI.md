# How Trident Consumes COSI

Trident uses a Composable Operating System Image ([COSI](../Reference/COSI.md))
file to provision the contents of file systems that are not being newly created
or adopted. The
[`source`](../Reference/Host-Configuration/API-Reference/FileSystemSource.md) of
the file system can be specified in the Host Configuration. A URL to the COSI
file as well as a hash of the COSI file's `metadata.json` file should be placed
in the [`image`
section](../Reference/Host-Configuration/API-Reference/OsImage.md) of the Host
Configuration.

This document explains the process Trident follows to read the COSI metadata and
stream the partition images to disk. For complete information on the structure
of a COSI file, please see the [COSI Reference Guide](../Reference/COSI.md).

## Important Requirements & Points of Caution

Trident checks for several requirements from the COSI URL to ensure that Trident
will be able to successfully read the COSI file.

1. If the COSI file is hosted on an HTTP server (`http://` or `https://`
   scheme), the server hosting the images **must** support HTTP Range Requests.
   This is necessary for Trident to read individual partition images from the
   COSI file. This functionality is elaborated on below. Trident will search for
   an `Accept-Ranges` header in the server's response, and will return an error
   if none is found. An explanation for why HTTP Range Requests are necessary
   can be found below under [Streaming and Verifying
   Images](#streaming-and-verifying-images).
2. If the COSI file is hosted in an OCI registry (`oci://` scheme), that registry
   **must** allow for anonymous image pulls. In addition, Trident expects that
   the referenced artifact contains exactly one layer.
3. Should your machine require a proxy to access a remoted hosted COSI image,
   ensure that the correct `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY`
   environment variables are set **and** are available to Trident at runtime. If
   running Trident from a container, ensure that the environment variables are
   passed to the container. Please see the [Docker CLI
   reference](https://docs.docker.com/reference/cli/docker/container/run/#env)
   on setting environment variables.

### Known Compatible Servers for Hosting COSI Files

- Azure Container Registry (use `oci://` scheme)

### Known Incompatible Servers for Hosting COSI Files

- Python3 `http.server` does not currently support HTTP Range Requests

## Reading the COSI Metadata

At the beginning of Trident's operations, Trident will load the COSI
`metadata.json` file into memory. Trident calculates the hash of the COSI
metadata. If a hash was provided in the [`image`
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
