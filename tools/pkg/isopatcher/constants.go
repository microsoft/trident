/*
Copyright © 2023 Microsoft Corporation
*/
package isopatcher

// MagicString is used to locate placeholder files in the initrd. Each placeholder file will be
// PlaceholderLengthBytes bytes long and start with this string, followed by the name
// of the file wrapped in colons. Unlike other files which may be compressed, each placeholder
// will directly have its bytes present in the output ISO so that it can be located and patched.
// This enables us to later replace the placeholder with the actual file contents without having
// to parse the ISO file format.
const MagicString = `#8505c8ab802dd717290331acd0592804c4e413b030150c53f5018ac998b7831d`
