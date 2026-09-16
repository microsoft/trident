# Print third-party license notices

Use `tailor notice` to print the license text needed when redistributing a tailor binary.

```bash
tailor notice
```

The command prints tailor's own MIT license first, then the third-party notices for every dependency
linked into the binary. It writes to stdout only; it does not create files by itself.

Redirect stdout when you need an attribution file for release or compliance archives:

```bash
tailor notice > THIRD-PARTY-NOTICES.txt
```

Related: see [Licensing](../explanation/licensing.md).
