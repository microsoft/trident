# Handle exit codes in scripts

Use tailor's exit codes to distinguish configuration problems from build failures in CI.

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `1` | Build or operational failure, such as an engine, IO, resolution, or signing error. |
| `2` | Usage or configuration error, such as bad arguments, an invalid selector, an unknown image, or a dependency cycle. |
| `130` | Interrupted run, such as Ctrl+C or SIGTERM cancellation (`128 + SIGINT`). |

Branch on `$?` after a failed command:

```bash
if tailor build gizmo; then
  echo "build succeeded"
else
  code=$?
  case "$code" in
    2)
      echo "fix tailor.yaml or the build arguments"
      ;;
    1)
      echo "build failed; inspect the build log"
      ;;
    130)
      echo "build interrupted"
      ;;
  esac
  exit "$code"
fi
```

Related: see the [CLI reference](../reference/cli.md).
