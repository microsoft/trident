#!/bin/python3

import sys
import hashlib
import tarfile

if len(sys.argv) != 2:
    raise ValueError("Usage: python3 cosi-sha384.py <COSI file>")

with tarfile.open(sys.argv[1], "r") as tar:
    m = tar.extractfile("metadata.json")
    if m is None:
        raise ValueError("metadata.json not found in COSI file")

    sha384 = hashlib.sha384()
    sha384.update(m.read())
    print(sha384.hexdigest())
