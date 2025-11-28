#!/usr/bin/python3

import sys

MAGIC_STRING = b"#8505c8ab802dd717290331acd0592804c4e413b030150c53f5018ac998b7831d"

if len(sys.argv) < 4:
    print("Usage: patch.py <input-iso> <output-iso> <trident-config>")
    sys.exit(1)

contents = bytearray(open(sys.argv[1], "rb").read())
patch = open(sys.argv[3], "rb").read()

placeholder = MAGIC_STRING + b":/etc/trident/config.yaml:"

index = contents.find(placeholder)
if index == -1:
    print("Input ISO does not contain the magic string")
    sys.exit(1)

placeholderLength = contents[index:].find(b"\n") + 1
if len(patch) > placeholderLength:
    print("Patch is too large")
    sys.exit(1)

if placeholderLength > len(patch):
    patch += b"\n"

if placeholderLength > len(patch):
    padding = placeholderLength - len(patch)
    patch += b"#" * (padding - 1) + b"\n"

contents[index : index + placeholderLength] = patch
open(sys.argv[2], "wb").write(contents)
